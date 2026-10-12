/// "Chat about this": a side panel next to the open thread to think a reply or a decision
/// through with the user's AI. Answers stream in; proposals under them are one click each.
import MermozKit
import SwiftUI

struct ChatPanel: View {
    @Environment(AppModel.self) private var model
    let thread: ThreadSummary
    @State private var draft = ""
    @FocusState private var focused: Bool

    private var messages: [ChatMessage] { model.chats[thread.id] ?? [] }
    private var streaming: String? { model.chatStreaming[thread.id] }

    /// Starting points for people who don't know what to ask yet.
    private let starters = ["What should I do with this?", "Help me write an answer", "Is this legit?", "What do they want from me?"]

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.line)
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        if messages.isEmpty && streaming == nil { intro }
                        ForEach(messages) { m in ChatBubble(message: m, threadId: thread.id) }
                        if let s = streaming { StreamingBubble(text: s) }
                        if let e = model.chatErrors[thread.id] {
                            Label(e, systemImage: "exclamationmark.triangle")
                                .font(.system(size: 12)).foregroundStyle(Theme.danger)
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                    .padding(14)
                }
                .onChange(of: messages.count) { proxy.scrollTo("end", anchor: .bottom) }
                .onChange(of: streaming) { proxy.scrollTo("end", anchor: .bottom) }
            }
            Divider().overlay(Theme.line)
            input
        }
        .foregroundStyle(Theme.text)
        .task(id: thread.id) {
            model.loadChat(thread.id)
            try? await Task.sleep(for: .milliseconds(50))
            focused = true
        }
    }

    private var header: some View {
        @Bindable var model = model
        return HStack(spacing: 8) {
            Image(systemName: "bubble.left.and.text.bubble.right").foregroundStyle(Theme.accent)
            Text("Chat about this").font(.system(size: 13, weight: .semibold))
            Spacer()
            if model.chatResearchAvailable[thread.id] ?? false {
                Button { model.chatResearch.toggle() } label: { Image(systemName: "globe") }
                    .buttonStyle(.plain)
                    .foregroundStyle(model.chatResearch ? Theme.accent : Theme.text3)
                    .help(model.chatResearch ? "Your AI may look things up on the web (click to turn off)" : "Let your AI look things up on the web")
                    .accessibilityLabel("Look things up on the web")
            }
            Button { model.clearChat(thread.id) } label: { Image(systemName: "arrow.counterclockwise") }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .disabled(messages.isEmpty || streaming != nil)
                .help("Start over").accessibilityLabel("Start over")
            Button { model.chatOpen = false } label: { Image(systemName: "xmark") }
                .buttonStyle(.plain).foregroundStyle(Theme.text2)
                .help("Close (Esc)").accessibilityLabel("Close chat")
        }
        .padding(.horizontal, 14).frame(height: 50)
    }

    private var intro: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Not sure what to do or what to say? Talk it through with your AI. It reads this conversation; nothing is sent or changed unless you click.")
                .font(.system(size: 12.5)).foregroundStyle(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(starters, id: \.self) { s in
                Button { model.sendChat(s, thread: thread.id) } label: {
                    Text(s).font(.system(size: 12.5)).padding(.horizontal, 10).padding(.vertical, 6)
                        .background(Theme.accentSoft.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain).foregroundStyle(Theme.accent)
            }
        }
    }

    private var input: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Ask anything about this conversation…", text: $draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .lineLimit(1...6)
                .focused($focused)
                .onSubmit(send)
            Button(action: send) {
                Image(systemName: "arrow.up.circle.fill").font(.system(size: 20))
            }
            .buttonStyle(.plain)
            .foregroundStyle(canSend ? Theme.accent : Theme.text3)
            .disabled(!canSend)
            .help("Send (↩)").accessibilityLabel("Send")
        }
        .padding(12)
    }

    private var canSend: Bool {
        streaming == nil && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func send() {
        guard canSend else { return }
        model.sendChat(draft, thread: thread.id)
        draft = ""
    }
}

/// One turn: the user's question on the right, the AI's answer (light Markdown, like the
/// Optimized view) on the left with its proposals underneath.
struct ChatBubble: View {
    @Environment(AppModel.self) private var model
    let message: ChatMessage
    let threadId: String

    var body: some View {
        if message.isAssistant {
            VStack(alignment: .leading, spacing: 8) {
                Text(CleanText.attributed(message.text))
                    .font(.system(size: 13))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if message.searched {
                    Label("Looked this up on the web", systemImage: "globe").font(.system(size: 11)).foregroundStyle(Theme.text3)
                } else if message.researchUnavailable {
                    Label("Your AI couldn't look this up on the web", systemImage: "globe").font(.system(size: 11)).foregroundStyle(Theme.text3)
                }
                ForEach(Array(message.actions.enumerated()), id: \.offset) { i, a in
                    ActionChip(action: a, key: "\(message.id)#\(i)", threadId: threadId)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            HStack {
                Spacer(minLength: 32)
                Text(message.text)
                    .font(.system(size: 13))
                    .textSelection(.enabled)
                    .padding(.horizontal, 11).padding(.vertical, 7)
                    .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 12))
            }
        }
    }
}

/// A proposal from the AI; nothing happens until it is clicked, and every change can be undone.
struct ActionChip: View {
    @Environment(AppModel.self) private var model
    let action: ChatAction
    let key: String
    let threadId: String

    private var done: Bool { model.chatApplied.contains(key) }

    var body: some View {
        Button { model.apply(action, key: key, thread: threadId) } label: {
            HStack(spacing: 6) {
                Image(systemName: done ? "checkmark" : action.symbol).font(.system(size: 11))
                Text(action.label).font(.system(size: 12, weight: .medium)).lineLimit(1)
            }
            .padding(.horizontal, 10).frame(height: 26)
            .background(done ? Theme.line : Theme.accentSoft, in: Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(done ? Theme.text3 : Theme.accent)
        // A reply can be reopened; a status or project change is done once.
        .disabled(done && action.kind != "draft_reply")
        .help(action.kind == "draft_reply" ? "Opens the reply, ready to edit. Nothing is sent." : action.label)
    }
}

/// The answer as it arrives; dots until the first words.
struct StreamingBubble: View {
    let text: String

    var body: some View {
        if text.isEmpty {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Thinking…").font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
        } else {
            Text(CleanText.attributed(text))
                .font(.system(size: 13))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
