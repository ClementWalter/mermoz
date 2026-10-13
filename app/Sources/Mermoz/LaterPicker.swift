/// The Later picker: quick choices with one-key shortcuts, a field that reads plain words
/// ("vendredi soir", "friday 7pm", "dans 3 jours") and shows when that is while typing,
/// and a date and time picker. A popover on a Feed card, a small panel elsewhere.
import MermozKit
import SwiftUI

struct LaterPicker: View {
    @Environment(AppModel.self) private var model
    let request: LaterRequest
    @State private var options: [LaterChoice] = []
    @State private var text = ""
    @State private var parsed: LaterTime?
    @State private var parseTask: Task<Void, Never>?
    @State private var picking = false
    @State private var day = Calendar.current.date(byAdding: .day, value: 1, to: Date()) ?? Date()
    @FocusState private var focus: Field?

    enum Field: Hashable { case text, choices }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Later").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text2)
            TextField("When? vendredi soir, friday 7pm, dans 3 jours…", text: $text)
                .textFieldStyle(.roundedBorder)
                .focused($focus, equals: .text)
                .onSubmit { confirmTyped() }
                .onChange(of: text) { _, t in parse(t) }
            if !text.trimmingCharacters(in: .whitespaces).isEmpty {
                Group {
                    if let p = parsed, p.until != nil {
                        Text("→ \(p.when)").foregroundStyle(Theme.accent)
                    } else {
                        Text("Say a day or a time, e.g. lundi matin or 15 nov").foregroundStyle(Theme.text3)
                    }
                }
                .font(.system(size: 12))
            }
            Divider()
            VStack(spacing: 2) {
                ForEach(options) { o in
                    row(o.label, detail: o.when, key: o.key) { choose(o) }
                }
                row("Pick a date & time…", detail: nil, key: LaterKeys.pick) { picking.toggle() }
            }
            .focusable()
            .focused($focus, equals: .choices)
            .focusEffectDisabled()
            .onKeyPress(phases: .down) { press in
                let k = press.characters.lowercased()
                if k == LaterKeys.pick { picking.toggle(); return .handled }
                if let o = LaterKeys.choice(for: k, in: options) { choose(o); return .handled }
                return .ignored
            }
            if picking {
                DatePicker("", selection: $day, in: Date()..., displayedComponents: [.date, .hourAndMinute])
                    .datePickerStyle(.graphical).labelsHidden()
                HStack {
                    Spacer()
                    Button("Later") {
                        let when = day.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).hour().minute())
                        model.putOff(request, until: FeedLater.millis(day), when: when, label: when)
                    }
                    .buttonStyle(.glassProminent).tint(Theme.accent)
                }
            }
        }
        .padding(14)
        .frame(width: 300)
        .task {
            options = await model.laterOptions()
            // ⇧L lands on the field; l on the choices, where one key picks.
            focus = request.typing ? .text : .choices
        }
    }

    private func row(_ title: String, detail: String?, key: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Text(title).font(.system(size: 13))
                Spacer()
                if let detail { Text(detail).font(.system(size: 12)).foregroundStyle(Theme.text3) }
                KeyHint(key)
            }
            .padding(.horizontal, 6).padding(.vertical, 5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func parse(_ t: String) {
        parseTask?.cancel()
        parseTask = Task {
            try? await Task.sleep(for: .milliseconds(80))
            guard !Task.isCancelled else { return }
            parsed = await model.laterParse(t)
        }
    }

    private func choose(_ o: LaterChoice) {
        guard let until = o.until else { return }
        model.putOff(request, until: until, when: o.when, label: o.label)
    }

    private func confirmTyped() {
        guard let p = parsed, let until = p.until else { return }
        model.putOff(request, until: until, when: p.when, label: p.when)
    }
}

/// The picker away from the Feed (List, Board): a small panel over the window.
struct LaterPanel: View {
    @Environment(AppModel.self) private var model
    let request: LaterRequest

    var body: some View {
        ZStack {
            Color(hex: "#D6E6F4").opacity(0.45).ignoresSafeArea()
                .onTapGesture { model.later = nil }
            LaterPicker(request: request)
                .glassEffect(.regular.tint(Color.white.opacity(0.7)), in: RoundedRectangle(cornerRadius: 16))
                .onExitCommand { model.later = nil }
        }
    }
}
