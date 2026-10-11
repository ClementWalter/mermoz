/// Plain-text composer with recipient pills that carry a trust mark.
import MermozKit
import SwiftUI

struct ComposeView: View {
    @Environment(AppModel.self) private var model
    @State var context: ComposeContext
    @State private var thenWaiting = false
    @State private var sending = false
    @State private var confirmSuspicious = false
    @FocusState private var bodyFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                if let tid = context.threadId, let t = model.detail?.thread, t.id == tid {
                    Text(t.number).font(.system(size: 11, design: .monospaced)).foregroundStyle(Theme.text3)
                    Text("Replying to \(t.lastFrom)").font(.system(size: 12.5)).foregroundStyle(Theme.text2).lineLimit(1)
                } else {
                    Text(context.draftId != nil ? "Edit draft" : "New message").font(.system(size: 13, weight: .semibold))
                }
                Spacer()
                Button { model.compose = nil } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).foregroundStyle(Theme.text2)
                    .keyboardShortcut(.cancelAction).accessibilityLabel("Discard")
            }
            .padding(.horizontal, 20).frame(height: 50)
            Divider().overlay(Theme.line)

            VStack(spacing: 0) {
                if context.threadId == nil && context.draftId == nil && model.accounts.count > 1 {
                    field("From") {
                        Picker("", selection: Binding(get: { context.accountId ?? "" }, set: { context.accountId = $0 })) {
                            ForEach(model.accounts) { Text($0.email).tag($0.id) }
                        }
                        .labelsHidden().fixedSize()
                    }
                }
                field("To") { RecipientField(addresses: $context.to) }
                field("Cc") { RecipientField(addresses: $context.cc) }
                field("Subject") {
                    TextField("", text: $context.subject).textFieldStyle(.plain).font(.system(size: 13))
                }
            }
            .padding(.horizontal, 22)

            TextEditor(text: $context.body)
                .font(.system(size: 14))
                .lineSpacing(4)
                .scrollContentBackground(.hidden)
                .focused($bodyFocused)
                .padding(12)
                .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.line))
                .padding(.horizontal, 22).padding(.top, 14)

            HStack(spacing: 10) {
                Button {
                    trySend()
                } label: {
                    HStack(spacing: 6) {
                        if sending { ProgressView().controlSize(.mini) }
                        Text(context.draftId != nil && model.isReview ? "Approve & send" : "Send")
                    }
                    .padding(.horizontal, 6)
                }
                .buttonStyle(.glassProminent)
                .tint(Theme.accent)
                .keyboardShortcut(.return, modifiers: .command)
                .disabled(sending || context.to.isEmpty)
                if context.threadId != nil {
                    Toggle("Then set Waiting", isOn: $thenWaiting).toggleStyle(.checkbox).font(.system(size: 12.5))
                }
                Spacer()
                Text("Plain text").font(.system(size: 12)).foregroundStyle(Theme.text3)
            }
            .padding(.horizontal, 22).padding(.vertical, 16)
        }
        .foregroundStyle(Theme.text)
        .onAppear { bodyFocused = !context.to.isEmpty }
        .confirmationDialog("Some recipients look suspicious", isPresented: $confirmSuspicious) {
            Button("Send anyway", role: .destructive) { doSend() }
            Button("Review recipients", role: .cancel) {}
        } message: {
            Text(suspicious.joined(separator: ", "))
        }
    }

    private var suspicious: [String] {
        (context.to + context.cc).filter {
            if case .suspicious = model.checker.check($0) { return true }
            return false
        }
    }

    private func trySend() {
        if suspicious.isEmpty { doSend() } else { confirmSuspicious = true }
    }

    private func doSend() {
        sending = true
        Task {
            _ = await model.send(context, thenWaiting: thenWaiting)
            sending = false
        }
    }

    private func field<C: View>(_ label: String, @ViewBuilder content: () -> C) -> some View {
        HStack(alignment: .center, spacing: 10) {
            Text(label).font(.system(size: 13)).foregroundStyle(Theme.text3).frame(width: 56, alignment: .leading)
            content()
            Spacer(minLength: 0)
        }
        .frame(minHeight: 38)
        .overlay(alignment: .bottom) { Rectangle().fill(Color(hex: "#E3EDF6")).frame(height: 1) }
    }
}

/// Pills plus an inline text field; commits on comma, space, return or blur.
struct RecipientField: View {
    @Environment(AppModel.self) private var model
    @Binding var addresses: [String]
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        FlowRow(spacing: 6) {
            ForEach(addresses, id: \.self) { a in
                RecipientPill(address: a, verdict: model.checker.check(a)) {
                    addresses.removeAll { $0 == a }
                }
            }
            TextField(addresses.isEmpty ? "name@example.com" : "", text: $draft)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .frame(minWidth: 140)
                .focused($focused)
                .onSubmit(commit)
                .onChange(of: draft) { _, v in
                    if v.hasSuffix(",") || v.hasSuffix(" ") || v.hasSuffix(";") { commit() }
                }
                .onChange(of: focused) { _, f in if !f { commit() } }
                .onKeyPress(.delete) {
                    if draft.isEmpty, !addresses.isEmpty { addresses.removeLast(); return .handled }
                    return .ignored
                }
        }
        .padding(.vertical, 6)
    }

    private func commit() {
        let parts = draft.split(whereSeparator: { ",; ".contains($0) }).map { String($0).trimmingCharacters(in: .whitespaces) }
        for p in parts where !p.isEmpty && !addresses.contains(p) { addresses.append(p) }
        draft = ""
    }
}

struct RecipientPill: View {
    let address: String
    let verdict: RecipientVerdict
    let onRemove: () -> Void
    @State private var showReason = false

    var body: some View {
        HStack(spacing: 5) {
            Text(address).lineLimit(1)
            mark
            Button(action: onRemove) { Image(systemName: "xmark").font(.system(size: 8, weight: .bold)) }
                .buttonStyle(.plain).foregroundStyle(Theme.text3).accessibilityLabel("Remove \(address)")
        }
        .font(.system(size: 12.5))
        .foregroundStyle(isBad ? Color(hex: "#9A2B2B") : Theme.text)
        .padding(.leading, 10).padding(.trailing, 8)
        .frame(height: 24)
        .background(isBad ? Color(hex: "#FFF7F7") : Color.white.opacity(0.9), in: Capsule())
        .overlay(Capsule().strokeBorder(isBad ? Color(hex: "#F3D3D3") : Color(hex: "#DCE8F3")))
        .onHover { h in if isBad { showReason = h } }
        .onTapGesture { if isBad { showReason.toggle() } }
        .popover(isPresented: $showReason, arrowEdge: .bottom) {
            if case let .suspicious(reason) = verdict {
                VStack(alignment: .leading, spacing: 3) {
                    Text("This address looks suspicious").font(.system(size: 12, weight: .semibold)).foregroundStyle(Color(hex: "#9A2B2B"))
                    Text(reason).font(.system(size: 12)).foregroundStyle(Color(hex: "#5B2020"))
                }
                .padding(11).frame(width: 260, alignment: .leading)
            }
        }
    }

    private var isBad: Bool {
        if case .suspicious = verdict { return true }
        return false
    }

    @ViewBuilder private var mark: some View {
        switch verdict {
        case .known:
            Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.ok)
                .accessibilityLabel("Address looks good")
        case .unknown:
            EmptyView()
        case .suspicious:
            Image(systemName: "exclamationmark.circle").font(.system(size: 11, weight: .medium)).foregroundStyle(Theme.danger)
                .accessibilityLabel("Suspicious address")
        }
    }
}
