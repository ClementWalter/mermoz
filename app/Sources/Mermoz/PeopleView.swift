/// People: the directory the agent builds from email signatures, with each person's threads.
import AppKit
import MermozKit
import SwiftUI

struct PeopleListView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "person.2").foregroundStyle(Theme.text2)
                Text("People").font(.system(size: 15, weight: .semibold))
                Text(model.contacts.count.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
                Spacer()
                if model.peopleJob.running {
                    ProgressView().controlSize(.mini)
                    Text("Tidying \(model.peopleJob.done)/\(model.peopleJob.total)").font(.system(size: 11.5)).foregroundStyle(Theme.text2)
                } else {
                    // ✦ merges duplicates and cleans names with AI.
                    Button { model.aiCleanPeople() } label: { Image(systemName: "sparkles") }
                        .buttonStyle(.plain).foregroundStyle(Theme.accent)
                        .help("Merge duplicates and clean names with AI")
                        .accessibilityLabel("Clean up with AI")
                }
            }
            .padding(.horizontal, 16).frame(height: 50)
            TextField("Search name, email or company", text: $model.contactQuery)
                .textFieldStyle(.plain)
                .padding(.horizontal, 10).frame(height: 30)
                .background(Theme.accent.opacity(0.07), in: RoundedRectangle(cornerRadius: 9))
                .padding(.horizontal, 12).padding(.bottom, 8)
                .task(id: model.contactQuery) {
                    try? await Task.sleep(for: .milliseconds(120))
                    await model.loadContacts()
                }
            Divider().overlay(Theme.line)
            if model.contacts.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "person.crop.rectangle.stack").font(.system(size: 24, weight: .light))
                    Text("No one yet").font(.system(size: 13, weight: .semibold))
                    Text("People you write to appear here. Turn on “Clean messages & build directory” in Settings › Agents to add their titles, companies and phone numbers from signatures.")
                        .font(.system(size: 12)).multilineTextAlignment(.center).frame(maxWidth: 280)
                }
                .foregroundStyle(Theme.text3)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(model.contacts) { c in
                            ContactRow(contact: c, selected: model.selectedContact?.contact.email == c.email)
                                .onTapGesture { model.openContact(c.email) }
                        }
                    }
                }
            }
        }
        .listPanel()
    }
}

struct ContactRow: View {
    let contact: Contact
    let selected: Bool

    var body: some View {
        HStack(spacing: 10) {
            Avatar(name: contact.display, key: contact.email.lowercased())
            VStack(alignment: .leading, spacing: 2) {
                Text(contact.display).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                Text(contact.role.isEmpty ? contact.email : contact.role).font(.system(size: 12)).foregroundStyle(Theme.text2).lineLimit(1)
            }
            Spacer(minLength: 4)
            VStack(alignment: .trailing, spacing: 2) {
                Text(Format.short(contact.lastSeen)).font(.system(size: 11.5)).foregroundStyle(Theme.text3)
                if let n = contact.conversations { Text(n == 1 ? "1 conv." : "\(n) conv.").font(.system(size: 10.5)).foregroundStyle(Theme.text3) }
            }
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 16).padding(.vertical, 9)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selected ? Theme.accent.opacity(0.11) : .clear)
        .overlay(alignment: .top) { Rectangle().fill(Theme.line.opacity(0.8)).frame(height: 1) }
        .contentShape(Rectangle())
    }
}

struct ContactDetailPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let d = model.selectedContact {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        HStack(spacing: 14) {
                            Avatar(name: d.contact.display, key: d.contact.email.lowercased()).scaleEffect(1.6).frame(width: 48, height: 48)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(d.contact.display).font(.system(size: 19, weight: .semibold))
                                if !d.contact.role.isEmpty { Text(d.contact.role).foregroundStyle(Theme.text2) }
                            }
                        }
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(d.contact.emails ?? [d.contact.email], id: \.self) { e in
                                field("envelope", e, url: URL(string: "mailto:\(e)"))
                            }
                            ForEach(d.contact.phones, id: \.self) { field("phone", $0, url: URL(string: "tel:\($0.filter { !$0.isWhitespace })")) }
                            if !d.contact.address.isEmpty { field("mappin.and.ellipse", d.contact.address, url: nil) }
                            if !d.contact.website.isEmpty { field("globe", d.contact.website, url: URL(string: d.contact.website.hasPrefix("http") ? d.contact.website : "https://\(d.contact.website)")) }
                            ForEach(d.contact.socials, id: \.self) { field("link", $0, url: URL(string: $0)) }
                        }
                        .padding(14)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
                        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#E3EDF6")))
                        Text("Conversations").font(.system(size: 11, weight: .semibold)).kerning(0.6).textCase(.uppercase).foregroundStyle(Theme.text3)
                        VStack(spacing: 0) {
                            ForEach(d.threads) { t in
                                ThreadRow(thread: t)
                                    .onTapGesture { model.reveal(t) }
                            }
                        }
                        .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                    }
                    .padding(26)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                VStack(spacing: 8) {
                    Image(systemName: "person.crop.circle").font(.system(size: 28, weight: .light)).foregroundStyle(Theme.text3)
                    Text("Select someone").foregroundStyle(Theme.text3)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .foregroundStyle(Theme.text)
        .paperPanel()
    }

    private func field(_ icon: String, _ text: String, url: URL?) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon).frame(width: 16).foregroundStyle(Theme.text3)
            if let url {
                Button(text) { NSWorkspace.shared.open(url) }.buttonStyle(.plain).foregroundStyle(Theme.accent)
            } else {
                Text(text)
            }
        }
        .font(.system(size: 13))
        .textSelection(.enabled)
    }
}
