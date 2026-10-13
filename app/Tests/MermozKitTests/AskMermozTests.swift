/// "Ask Mermoz to do": when typed text is an instruction, the typed delete confirmation,
/// and the daemon's preview decoded as it is sent.
import Foundation
import Testing
@testable import MermozKit

@Test(arguments: [
    "archive all messages past 5 months not in done",
    "delete everything from shop.example",
    "mark all newsletters as read",
    "Déplace les factures dans Compta",
    "move travel mail to done",
])
func anImperativeSentenceIsAnInstruction(text: String) {
    #expect(AskIntent.isInstruction(text, otherMatches: true))
}

@Test(arguments: ["archive", "invoice", "marc audit"])
func shortTextIsASearch(text: String) {
    #expect(!AskIntent.isInstruction(text, otherMatches: false))
}

@Test func aLongSearchWithResultsStaysASearch() {
    #expect(!AskIntent.isInstruction("audit report from marc", otherMatches: true))
}

@Test func aLongSentenceWithNoMatchIsAnInstruction() {
    #expect(AskIntent.isInstruction("everything older than a year goes away", otherMatches: false))
}

@Test(arguments: ["1842", "1,842", " 1 842 "])
func theCountConfirmsADelete(typed: String) {
    #expect(AskIntent.confirms(typed, total: 1842))
}

@Test(arguments: ["", "184", "yes", "18420"])
func anythingElseDoesNot(typed: String) {
    #expect(!AskIntent.confirms(typed, total: 1842))
}

@Test func countsHaveThousandsSeparators() {
    #expect(AskIntent.count(1842) == "1,842")
}

private let previewJSON = """
{"kind":"plan","id":"bop_1","sentence":"archive all messages past 5 months not in done",
 "plan":{"steps":[]},"summary":"Archive 1,842 threads older than 5 months that aren't Done",
 "interpretation":"status ≠ Done AND older than 5 months",
 "steps":[{"summary":"Archive 1,842 threads older than 5 months that aren't Done",
   "interpretation":"status ≠ Done AND older than 5 months","action":"Archive","count":1842,
   "by_status":[{"status":"todo","name":"To do","count":1200}],
   "sample":[{"id":"thr_1","number":"M-12","status":"todo","subject":"Ferry tickets","last_from":"Nora Lind","last_date":1}]}],
 "total":1842,"deletes":false,"warning":null,"confirm":"enter"}
"""

@Test func aPreviewDecodesAsTheDaemonSendsIt() throws {
    let p = try RPCCoding.decoder().decode(DoPreview.self, from: Data(previewJSON.utf8))
    #expect(p.steps?.first?.sample.first?.lastFrom == "Nora Lind")
}

@Test func aPreviewIsAPlan() throws {
    let p = try RPCCoding.decoder().decode(DoPreview.self, from: Data(previewJSON.utf8))
    #expect(p.isPlan)
}

@Test func aQuestionIsNotAPlan() throws {
    let q = #"{"kind":"question","message":"I can't send email."}"#
    let p = try RPCCoding.decoder().decode(DoPreview.self, from: Data(q.utf8))
    #expect(!p.isPlan)
}
