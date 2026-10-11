/// Review lane: emails an agent wants to send, waiting for the user's approval.
import MermozKit
import SwiftUI

struct ReviewListView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            ListHeader()
            Divider().overlay(Theme.line)
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(model.reviewDrafts) { d in
                        let on = model.selectedDraftId == d.id
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text("To \(d.to.joined(separator: ", "))").font(.system(size: 13, weight: .semibold)).lineLimit(1)
                                Spacer()
                                Text(Format.short(d.updated)).font(.system(size: 12)).foregroundStyle(Theme.text3)
                            }
                            Text(d.subject.isEmpty ? "(no subject)" : d.subject).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                            Chip(small: true) { Text("Drafted by your agent") }
                        }
                        .foregroundStyle(Theme.text)
                        .padding(.horizontal, 16).padding(.vertical, 10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(on ? Theme.accent.opacity(0.11) : .clear)
                        .overlay(alignment: .leading) { if on { Rectangle().fill(Theme.accent).frame(width: 3) } }
                        .overlay(alignment: .bottom) { Rectangle().fill(Theme.line.opacity(0.8)).frame(height: 1) }
                        .contentShape(Rectangle())
                        .onTapGesture { model.selectedDraftId = d.id; model.compose = nil }
                    }
                    Text("When an agent wants to send an email, it waits here. Nothing leaves without you.")
                        .font(.system(size: 12.5)).foregroundStyle(Theme.text3).multilineTextAlignment(.center)
                        .padding(24)
                }
            }
        }
        .listPanel()
    }
}

struct ReviewDetailView: View {
    @Environment(AppModel.self) private var model
    let draft: Draft

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: "checkmark.shield").font(.system(size: 17)).foregroundStyle(Color(hex: "#C2568C"))
                Text("An agent wants to send this email.").font(.system(size: 13, weight: .semibold)).foregroundStyle(Color(hex: "#3F1229"))
                Spacer()
            }
            .padding(.horizontal, 14).padding(.vertical, 12)
            .background(Color(hex: "#FBEFF5"), in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#F0CADC")))
            .padding([.horizontal, .top], 24)

            Text(draft.subject.isEmpty ? "(no subject)" : draft.subject)
                .font(.system(size: 18, weight: .semibold))
                .padding(.horizontal, 24).padding(.top, 16)

            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 6) {
                GridRow { Text("From").foregroundStyle(Theme.text2); Text(model.accounts.first { $0.id == draft.accountId }?.email ?? "") }
                GridRow { Text("To").foregroundStyle(Theme.text2); Text(draft.to.joined(separator: ", ")) }
                if !draft.cc.isEmpty { GridRow { Text("Cc").foregroundStyle(Theme.text2); Text(draft.cc.joined(separator: ", ")) } }
            }
            .font(.system(size: 12.5))
            .padding(.horizontal, 24).padding(.vertical, 10)
            Divider().overlay(Color(hex: "#E3EDF6")).padding(.horizontal, 24)

            ScrollView {
                Text(draft.body).font(.system(size: 13.5)).lineSpacing(4).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 24).padding(.vertical, 12)
            }

            Divider().overlay(Color(hex: "#E3EDF6")).padding(.horizontal, 24)
            HStack(spacing: 8) {
                Button("Approve & send") { model.approve(draft) }
                    .buttonStyle(.glassProminent).tint(Theme.accent)
                    .keyboardShortcut(.return, modifiers: .command)
                Button("Edit") { model.editDraft(draft) }.buttonStyle(.glass)
                Spacer()
                Button("Reject", role: .destructive) { model.reject(draft) }.buttonStyle(.glass)
            }
            .padding(.horizontal, 24).padding(.vertical, 16)
        }
        .foregroundStyle(Theme.text)
    }
}
