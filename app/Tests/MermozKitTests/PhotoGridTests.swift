/// Photos sent along with a message show in the chat as a grid under the bubble's text,
/// as in a messaging app: up to four tiles, the last one counting the rest, a click for
/// Quick Look. Images pasted in the body stay their own strip, never counted twice.
import AppKit
import Testing
@testable import MermozKit

private func photo(_ id: String, mime: String = "image/jpeg", name: String? = nil) -> MermozKit.Attachment {
    MermozKit.Attachment(id: id, filename: name ?? "\(id).jpg", mime: mime, size: 2_000_000)
}

private func message(_ id: String, attachments: [MermozKit.Attachment]) -> Message {
    Message(id: id, threadId: "t", from: Address(name: "Nora", email: "nora@friends.example"), to: [], cc: [], bcc: nil,
            date: 1_000, subject: "Photos", bodyText: "Here are the photos from Saturday.", bodyNew: nil, hasQuoted: nil,
            bodyClean: nil, hasHtml: false, isFromMe: false, attachments: attachments, auth: nil)
}

private func photos(_ n: Int) -> [MermozKit.Attachment] { (0..<n).map { photo("p\($0)") } }

private func bubble(_ m: Message) -> BubbleSpec? {
    ChatRows.build(items: [.message(m)], hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                   time: { _ in "09:41" }).rows.compactMap { if case let .bubble(b) = $0 { b } else { nil } }.first
}

// MARK: grid layout

@Test(arguments: [(1, 1), (2, 2), (3, 3), (5, 4)])
func theGridShowsAtMostFourTiles(photos: Int, tiles: Int) {
    #expect(ChatMetrics.photoTiles(min(photos, ChatMetrics.photosShown)).count == tiles)
}

@Test(arguments: [1, 2, 3, 4])
func tilesStayInsideTheGrid(count: Int) {
    #expect(ChatMetrics.photoTiles(count).allSatisfy { $0.minX >= 0 && $0.maxX <= ChatMetrics.photoGrid })
}

@Test(arguments: [2, 3, 4])
func noTwoTilesOverlap(count: Int) {
    let tiles = ChatMetrics.photoTiles(count)
    let overlaps = tiles.indices.flatMap { i in tiles.indices.filter { $0 > i && tiles[i].intersects(tiles[$0]) } }
    #expect(overlaps.isEmpty)
}

@Test func aBubbleWithPhotosIsTallerByTheGrid() {
    let plain = bubble(message("m", attachments: []))!
    let withPhotos = bubble(message("m", attachments: photos(3)))!
    #expect(ChatMetrics.bubble(text: CGSize(width: 200, height: 40), withPhotos)
        - ChatMetrics.bubble(text: CGSize(width: 200, height: 40), plain) == ChatMetrics.photosHeight(3))
}

// MARK: which files are photos

@Test(arguments: ["image/jpeg", "image/png", "image/heic", "image/gif", "image/webp", "IMAGE/JPEG"])
func anImageFileIsAPhoto(mime: String) {
    #expect(ChatPhotos.isPhoto(photo("a", mime: mime)))
}

@Test func aPDFIsNotAPhoto() {
    #expect(!ChatPhotos.isPhoto(photo("a", mime: "application/pdf", name: "avis.pdf")))
}

@Test func aGenericFileNamedLikeAPhotoIsAPhoto() {
    #expect(ChatPhotos.isPhoto(photo("a", mime: "application/octet-stream", name: "IMG_0412.HEIC")))
}

// MARK: pasted and attached, each once

private let mixed = message("m", attachments: [
    MermozKit.Attachment(id: "pasted", filename: "image001.png", mime: "image/png", size: 10_000, inline: true),
    photo("p1"), photo("p2"),
    MermozKit.Attachment(id: "doc", filename: "avis.pdf", mime: "application/pdf", size: 100_000),
])

@Test func attachedPhotosGoToTheGrid() {
    #expect(bubble(mixed)?.photos.map(\.id) == ["p1", "p2"])
}

@Test func aPastedImageStaysInItsStrip() {
    #expect(bubble(mixed)?.thumbnails == ["pasted"])
}

@Test func onlyOtherFilesAreCounted() {
    #expect(bubble(mixed)?.files == 1)
}

// MARK: in the stream

@MainActor
private final class Shown {
    let stream = ChatStreamView()
    let window = NSWindow(contentRect: CGRect(x: -10_000, y: -10_000, width: 700, height: 700), styleMask: .borderless,
                          backing: .buffered, defer: false)
    var previewed: ([URL], Int)?

    init(_ m: Message) {
        window.contentView = stream
        stream.actions.file = { id in URL(fileURLWithPath: "/tmp/\(id).jpg") }
        stream.actions.previewPhotos = { [weak self] urls, i in self?.previewed = (urls, i) }
        let rows = ChatRows.build(items: [.message(m)], hidden: 0, summary: nil, expanded: [], shownInFull: [], focused: nil,
                                  time: { _ in "09:41" }).rows
        stream.show(rows, conversation: "c", messages: 1)
        stream.layoutSubtreeIfNeeded()
        stream.documentView?.layoutSubtreeIfNeeded()
    }
}

@MainActor @Test func fivePhotosCountOneMoreOnTheLastTile() {
    let s = Shown(message("m", attachments: photos(5)))
    #expect(s.stream.bubbleRow("m")?.hiddenPhotos == 1)
}

@MainActor @Test func theGridSitsUnderTheText() {
    let s = Shown(message("m", attachments: photos(2)))
    let row = s.stream.bubbleRow("m")!
    #expect(row.photoRects.allSatisfy { $0.minY >= row.text.frame.maxY })
}

@MainActor @Test func theGridSitsInsideTheBubble() {
    let s = Shown(message("m", attachments: photos(3)))
    let row = s.stream.bubbleRow("m")!
    #expect(row.photoRects.allSatisfy { row.bubbleRect.contains($0) })
}

@MainActor @Test func clickingATileShowsThePhotosFromThatOne() async {
    let s = Shown(message("m", attachments: photos(3)))
    s.stream.bubbleRow("m")?.pressPhoto(2)
    for _ in 0..<50 where s.previewed == nil { try? await Task.sleep(for: .milliseconds(10)) }
    #expect(s.previewed?.1 == 2)
}

@MainActor @Test func quickLookGetsEveryPhotoOfTheMessage() async {
    let s = Shown(message("m", attachments: photos(5)))
    s.stream.bubbleRow("m")?.pressPhoto(3)
    for _ in 0..<50 where s.previewed == nil { try? await Task.sleep(for: .milliseconds(10)) }
    #expect(s.previewed?.0.count == 5)
}

// MARK: loading

/// Whether the decode ran on the main thread; nil until it ran.
private final class DecodeThread: @unchecked Sendable {
    private let lock = NSLock()
    private var main: Bool?
    func record() { lock.lock(); main = Thread.isMainThread; lock.unlock() }
    var onMain: Bool? { lock.lock(); defer { lock.unlock() }; return main }
}

@MainActor @Test func photosAreDecodedOffTheMainThread() async {
    let seen = DecodeThread()
    let thumbs = Thumbnails(decode: { _ in seen.record(); return nil })
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
        thumbs.load("p1", file: { _ in URL(fileURLWithPath: "/tmp/p1.jpg") }) { done.resume() }
    }
    #expect(seen.onMain == false)
}

@MainActor @Test func aFetchedPhotoKeepsItsFileForDragging() async {
    let thumbs = Thumbnails(decode: { _ in nil })
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
        thumbs.load("p1", file: { _ in URL(fileURLWithPath: "/tmp/p1.jpg") }) { done.resume() }
    }
    #expect(thumbs.url("p1")?.lastPathComponent == "p1.jpg")
}

@MainActor @Test func showingPhotosDoesNotWaitForThem() {
    let thumbs = Thumbnails(decode: { _ in nil })
    thumbs.load("slow", file: { _ in try? await Task.sleep(for: .seconds(5)); return nil }) {}
    #expect(thumbs.image("slow") == nil)
}
