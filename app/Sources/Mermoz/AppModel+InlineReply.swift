/// The reply composer at the bottom of a conversation's chat: r puts the keyboard there,
/// ⌘↩ sends through the usual send and approval path, Esc leaves it with the draft
/// kept. The full compose window opens only on request (expand, or the recipients).
import Foundation
import MermozKit

extension AppModel {
    /// The open conversation's inline reply: its draft, or a fresh reply-all.
    func inlineReply(_ d: ThreadDetail) -> ComposeContext {
        inlineReplies[d.thread.id] ?? replyContext(d)
    }

    /// Keeps what the inline composer holds for this conversation.
    func setInlineReply(_ c: ComposeContext, for d: ThreadDetail) {
        inlineReplies[d.thread.id] = c
    }

    /// r: the keyboard goes to the inline composer of the conversation on screen.
    func focusReply() {
        guard detail != nil else { return }
        if conversationTab != .messages { conversationTab = .messages }
        replyFocusRequest += 1
    }

    /// r: replies to the message in focus (↑/↓) when there is one, else to the conversation.
    func replyFromKeyboard() {
        if let id = focusedMessageId, detail?.messages.contains(where: { $0.id == id }) == true {
            pinReply(to: id)
        } else {
            focusReply()
        }
    }

    /// Slide to reply, ↩ on a bubble, or r on one in focus: the reply answers that
    /// message, addressed as a reply to it, and the keyboard goes to the composer. What
    /// was typed stays.
    func pinReply(to messageId: String) {
        guard let d = detail, let m = d.messages.first(where: { $0.id == messageId }) else { return }
        var c = inlineReply(d)
        let base = replyContext(d, answering: m)
        c.to = base.to
        c.cc = base.cc
        c.threadId = base.threadId
        c.subject = base.subject
        c.replyToMessageId = m.id
        setInlineReply(c, for: d)
        focusReply()
    }

    /// × on the pinned preview: the reply goes back to the conversation's own default.
    func unpinReply() {
        guard let d = detail else { return }
        var c = inlineReply(d)
        let base = replyContext(d)
        c.to = base.to
        c.cc = base.cc
        c.threadId = base.threadId
        c.subject = base.subject
        c.replyToMessageId = nil
        setInlineReply(c, for: d)
    }

    /// A reply to one message: to its Reply-To (else its sender), the others in copy; to
    /// what it was sent to when it is the user's own. In that message's Gmail thread.
    func replyContext(_ d: ThreadDetail, answering m: Message) -> ComposeContext {
        let me = accounts.first { $0.id == d.thread.accountId }?.email.lowercased()
        let to: [String], cc: [String]
        if m.isFromMe {
            to = m.to.map(\.email)
            cc = m.cc.map(\.email)
        } else {
            let sender = (m.replyTo?.isEmpty == false ? m.replyTo! : [m.from]).map(\.email)
            to = sender
            let skip = Set(sender.map { $0.lowercased() } + [m.from.email.lowercased(), me ?? ""])
            cc = (m.to + m.cc).map(\.email).filter { !skip.contains($0.lowercased()) }
        }
        let thread = m.threadId.flatMap { t in d.conversation?.threads.first { $0.id == t } } ?? d.thread
        let subj = thread.subject.lowercased().hasPrefix("re:") ? thread.subject : "Re: \(thread.subject)"
        return ComposeContext(threadId: thread.id, accountId: d.thread.accountId, to: to, cc: cc, subject: subj)
    }

    /// The pinned message, if the reply is pinned to one still in the conversation.
    func pinnedMessage(_ d: ThreadDetail) -> Message? {
        guard let id = inlineReply(d).replyToMessageId else { return nil }
        return d.messages.first { $0.id == id }
    }

    /// The full compose window, with what the inline composer held.
    func expandInlineReply() {
        guard let d = detail else { return }
        let c = inlineReply(d)
        inlineReplies[d.thread.id] = nil
        replyFocused = false
        compose = c
    }

    /// ⌘↩ in the inline composer: sends the reply; the draft goes once it is sent.
    @discardableResult
    func sendInlineReply() async -> Bool {
        guard let d = detail else { return false }
        let c = inlineReply(d)
        guard !c.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !c.attachments.items.isEmpty else { return false }
        // Never sent silently to an address nobody reads: the full window asks first.
        if c.noReplyWarning != nil {
            expandInlineReply()
            return false
        }
        switch await sendCompose(c) {
        case .sent, .stillSending:
            inlineReplies[d.thread.id] = nil
            return true
        case let .failed(again):
            // Kept with its error and draft: the bar says why, ⌘↩ retries.
            inlineReplies[d.thread.id] = again
            return false
        }
    }

    /// Files dropped or pasted on the inline composer join its draft (sizes read off the
    /// main thread; a folder is refused, as in the full window).
    func attachInline(_ urls: [URL], inline: Bool = false, to d: ThreadDetail) async {
        let found = await Task.detached(priority: .userInitiated) { () -> [(URL, Int64, Bool)] in
            urls.map { u in
                let v = try? u.resourceValues(forKeys: [.fileSizeKey, .isDirectoryKey])
                return (u, Int64(v?.fileSize ?? 0), v?.isDirectory ?? false)
            }
        }.value
        var c = inlineReply(d)
        for (url, size, isDir) in found {
            if isDir {
                toast = "\(url.lastPathComponent) is a folder. Compress it first, then attach the .zip file."
                continue
            }
            _ = c.attachments.add(ComposeAttachment(name: url.lastPathComponent, size: size, source: .file(url), inline: inline))
        }
        setInlineReply(c, for: d)
    }

    /// The names the conversation knows for its addresses, for the recipients line.
    func knownNames(_ d: ThreadDetail) -> [String: String] {
        var names: [String: String] = [:]
        for m in d.messages {
            for a in [m.from] + m.to + m.cc where !a.name.isEmpty { names[a.email.lowercased()] = a.name }
        }
        return names
    }

    /// Sends a composed message, or hands it to the test stub.
    func sendCompose(_ c: ComposeContext) async -> SendOutcome {
        if let sendOverride { return await sendOverride(c) ? .sent : .failed(c) }
        return await send(c, thenWaiting: false)
    }
}
