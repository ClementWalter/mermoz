/// A reply starts from the daemon's addressing (Reply-To honoured), warns about an
/// address nobody reads, never sends to one silently, and a failed send keeps the
/// composer with its error for Retry. Model only: no window.
@testable import MermozKit
import Testing
@testable import Mermoz

private let me = Address(name: "Me", email: "me@example.com")
private let noReply = Address(name: "Franck ADAM (via Invoices)", email: "no-reply@invoices.example")
private let franck = Address(name: "Franck ADAM", email: "franck@plomberie.example")

private func invoice(reply: ReplyAddressing?) -> ThreadDetail {
    let t = ThreadSummary(id: "t1", number: "M-1", accountId: "a", subject: "Votre facture", snippet: "", participants: [],
                          lastFrom: "Franck", lastDate: 0, unread: false, messageCount: 1, hasAttachments: false,
                          status: "todo", priority: 0)
    let m = Message(id: "m1", threadId: "t1", from: noReply, to: [me], cc: [], bcc: nil, replyTo: [franck], date: 0,
                    subject: "Votre facture", bodyText: "Votre facture.", bodyNew: nil, hasQuoted: nil, bodyClean: nil,
                    hasHtml: false, isFromMe: false, attachments: [], auth: nil)
    var d = ThreadDetail(thread: t, messages: [m])
    d.reply = reply
    return d
}

@MainActor
private func model() -> AppModel {
    let m = AppModel()
    m.accounts = [Account(id: "a", email: me.email, provider: "gmail", sync: SyncInfo(state: "idle", synced: 0, total: 0))]
    return m
}

@MainActor @Test func replyGoesWhereTheDaemonSays() {
    let c = model().replyContext(invoice(reply: ReplyAddressing(to: [franck], cc: [])))
    #expect(c.to == ["franck@plomberie.example"])
}

@MainActor @Test func replyToANoReplySenderCarriesTheWarning() {
    let c = model().replyContext(invoice(reply: ReplyAddressing(to: [noReply], cc: [], noReply: [noReply.email])))
    #expect(c.noReplyWarning != nil)
}

@MainActor @Test func warningOffersThePersonFoundInTheMail() {
    let r = ReplyAddressing(to: [noReply], cc: [], noReply: [noReply.email], suggestion: franck)
    #expect(model().replyContext(invoice(reply: r)).noReplyWarning?.question == "Did you mean franck@plomberie.example?")
}

@MainActor @Test func inlineReplyToANoReplyAddressOpensTheFullWindowInstead() async {
    let m = model()
    let d = invoice(reply: ReplyAddressing(to: [noReply], cc: [], noReply: [noReply.email]))
    m.detail = d
    var c = m.inlineReply(d)
    c.body = "Merci"
    m.setInlineReply(c, for: d)
    var sent = false
    m.sendOverride = { _ in sent = true; return true }
    _ = await m.sendInlineReply()
    #expect(!sent)
}

@MainActor @Test func inlineReplyToANoReplyAddressShowsTheFullWindow() async {
    let m = model()
    let d = invoice(reply: ReplyAddressing(to: [noReply], cc: [], noReply: [noReply.email]))
    m.detail = d
    var c = m.inlineReply(d)
    c.body = "Merci"
    m.setInlineReply(c, for: d)
    _ = await m.sendInlineReply()
    #expect(m.compose?.to == [noReply.email])
}

@MainActor @Test func failedSendKeepsTheComposerWithItsError() async {
    let m = model()
    let c = ComposeContext(threadId: "t1", accountId: "a", to: [franck.email], cc: [], subject: "Re: Votre facture",
                           body: "Merci")
    guard case let .failed(again) = await m.send(c, thenWaiting: false) else {
        Issue.record("a send without the daemon cannot succeed")
        return
    }
    #expect(again.sendError == "Mermoz isn't connected yet. Retry in a moment.")
}

@MainActor @Test func failedInlineReplyKeepsItsError() async {
    let m = model()
    let d = invoice(reply: ReplyAddressing(to: [franck], cc: []))
    m.detail = d
    var c = m.inlineReply(d)
    c.body = "Merci"
    m.setInlineReply(c, for: d)
    _ = await m.sendInlineReply()
    #expect(m.inlineReply(d).sendError != nil)
}
