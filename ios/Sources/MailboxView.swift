/// Phone navigation, local search and persisted workflow actions for demo mail.
import SwiftUI

struct MailboxView: View {
    @Bindable var model: MailboxModel
    @State private var query = ""
    @State private var status: String?
    @State private var showingInfo = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if query.isEmpty {
                    ScrollView(.horizontal) {
                        HStack(spacing: 8) {
                            statusButton("All", key: nil)
                            ForEach(model.statuses) { statusButton($0.name, key: $0.key) }
                        }
                        .padding(.horizontal)
                        .padding(.vertical, 10)
                    }
                    .scrollIndicators(.hidden)
                }
                if let error = model.error {
                    ContentUnavailableView("Couldn't load mail", systemImage: "exclamationmark.triangle", description: Text(error))
                } else if model.loading && model.threads.isEmpty {
                    ProgressView("Opening your local mailbox…").frame(maxHeight: .infinity)
                } else {
                    List(model.threads) { thread in
                        NavigationLink(value: thread) { ThreadRow(thread: thread) }
                            .accessibilityIdentifier("thread-\(thread.id)")
                    }
                    .listStyle(.plain)
                    .overlay {
                        if model.threads.isEmpty && !model.loading {
                            ContentUnavailableView(query.isEmpty ? "All clear" : "No matching mail", systemImage: "tray")
                        }
                    }
                    .refreshable { await model.load(status: status, query: query) }
                }
            }
            .navigationTitle("Mermoz")
            .searchable(text: $query, prompt: "Search your mail")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Demo mailbox", systemImage: "info.circle") { showingInfo = true }
                }
            }
            .safeAreaInset(edge: .bottom) {
                Text("Demo mailbox · Stored on this iPhone")
                    .font(.caption).foregroundStyle(.secondary)
                    .padding(10).frame(maxWidth: .infinity).background(.bar)
            }
            .navigationDestination(for: MailThread.self) { thread in
                ThreadView(model: model, thread: thread)
            }
            .task(id: "\(status ?? "all")|\(query)|\(model.revision)") {
                await model.load(status: status, query: query)
            }
            .sheet(isPresented: $showingInfo) {
                NavigationStack {
                    Form {
                        Section("Your local mailbox") {
                            Text("This first iPhone build contains fictional mail for Maya Laurent. Mail and status changes stay on this device and work offline.")
                        }
                        Section("Coming next") {
                            Text("Gmail sign-in, replying, attachments and AI features aren't connected in this build.")
                        }
                    }
                    .navigationTitle("Mermoz for iPhone")
                    .toolbar { Button("Done") { showingInfo = false } }
                }
            }
        }
        .tint(Color(red: 0.18, green: 0.36, blue: 0.29))
    }

    private func statusButton(_ title: String, key: String?) -> some View {
        Button { status = key } label: {
            Text(title).font(.subheadline.weight(.medium))
                .padding(.horizontal, 14).padding(.vertical, 8)
                .background(status == key ? Color.accentColor.opacity(0.14) : Color.secondary.opacity(0.08), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(status == key ? .isSelected : [])
    }
}

private struct ThreadRow: View {
    let thread: MailThread

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                if thread.unread { Circle().fill(.tint).frame(width: 7, height: 7).accessibilityLabel("Unread") }
                Text(thread.lastFrom).font(.subheadline.weight(.semibold)).lineLimit(1)
                Spacer()
                Text(Date(timeIntervalSince1970: Double(thread.lastDate) / 1000), style: .date)
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text(thread.subject).font(.headline).lineLimit(2)
            Text(thread.snippet).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
            HStack {
                Text(thread.status.capitalized)
                if thread.messageCount > 1 { Text("· \(thread.messageCount) messages") }
            }
            .font(.caption).foregroundStyle(.secondary)
        }
        .padding(.vertical, 8)
    }
}

private struct ThreadView: View {
    @Bindable var model: MailboxModel
    let thread: MailThread
    @State private var detail: MailDetail?
    @State private var error: String?
    @State private var updating = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Text(thread.subject).font(.title2.bold()).accessibilityIdentifier("thread-subject")
                if let detail {
                    ForEach(detail.messages) { message in
                        VStack(alignment: .leading, spacing: 12) {
                            HStack(alignment: .top) {
                                VStack(alignment: .leading) {
                                    Text(message.from.display).font(.headline)
                                    Text(message.from.email).font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                Text(Date(timeIntervalSince1970: Double(message.date) / 1000), style: .date)
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Text(message.bodyText).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .padding().background(Color.secondary.opacity(0.07), in: RoundedRectangle(cornerRadius: 16))
                    }
                } else if error == nil { ProgressView("Opening conversation…") }
                if let error { Text(error).foregroundStyle(.red).accessibilityIdentifier("mail-error") }
            }
            .padding()
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            Menu {
                ForEach(model.statuses) { status in
                    Button(status.name) { Task { await update(status.key) } }
                }
            } label: {
                Label(detail?.thread.status.capitalized ?? thread.status.capitalized, systemImage: "tag")
            }
            .disabled(updating || detail == nil)
            .accessibilityIdentifier("thread-status")
        }
        .task {
            do { detail = try await model.mailbox.call("threads.get", params: ["id": thread.id]) }
            catch { self.error = error.localizedDescription }
        }
    }

    private func update(_ status: String) async {
        updating = true
        defer { updating = false }
        do {
            let _: MailThread = try await model.mailbox.call("threads.update", params: ["id": thread.id, "status": status])
            detail = try await model.mailbox.call("threads.get", params: ["id": thread.id])
            error = nil
            model.revision += 1
        } catch { self.error = error.localizedDescription }
    }
}
