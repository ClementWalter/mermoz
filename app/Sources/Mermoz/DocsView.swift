/// Docs: every file shared in any conversation, newest first. Opening one shows its
/// conversation on the right; double-clicking opens the file.
import MermozKit
import SwiftUI

struct DocsListView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "doc.on.doc").foregroundStyle(Theme.text2)
                Text("Docs").font(.system(size: 15, weight: .semibold))
                Text(model.files.count.formatted()).font(.system(size: 12)).foregroundStyle(Theme.text3)
                Spacer()
            }
            .padding(.horizontal, 16).frame(height: 50)
            TextField("Search file names", text: $model.fileQuery)
                .textFieldStyle(.plain)
                .padding(.horizontal, 10).frame(height: 30)
                .background(Theme.accent.opacity(0.07), in: RoundedRectangle(cornerRadius: 9))
                .padding(.horizontal, 12).padding(.bottom, 8)
                .task(id: model.fileQuery) {
                    try? await Task.sleep(for: .milliseconds(120))
                    await model.loadFiles()
                }
            Divider().overlay(Theme.line)
            if model.files.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "doc").font(.system(size: 24, weight: .light))
                    Text("No files yet").font(.system(size: 13, weight: .semibold))
                    Text("Files sent or received in your conversations appear here.")
                        .font(.system(size: 12)).multilineTextAlignment(.center).frame(maxWidth: 280)
                }
                .foregroundStyle(Theme.text3)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(model.files) { f in
                            FileRow(file: f, selected: model.selectedThreadId == f.threadId)
                                .onTapGesture(count: 2) { model.openAttachment(f.attachment) }
                                .onTapGesture { model.openFileConversation(f) }
                        }
                    }
                }
            }
        }
        .listPanel()
    }
}

/// One shared file: its type, name, who shared it and when.
struct FileRow: View {
    let file: SharedFile
    var selected = false

    var body: some View {
        HStack(spacing: 10) {
            AttachmentChip(attachment: file.attachment)
            VStack(alignment: .leading, spacing: 2) {
                Text(file.from).font(.system(size: 12, weight: .medium)).lineLimit(1)
                Text(Format.short(file.date)).font(.system(size: 11.5)).foregroundStyle(Theme.text3)
            }
            Spacer(minLength: 0)
        }
        .foregroundStyle(Theme.text)
        .padding(.horizontal, 14).padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selected ? Theme.accent.opacity(0.11) : .clear)
        .contentShape(Rectangle())
        .help("Click to see the conversation · double-click to open the file")
    }
}
