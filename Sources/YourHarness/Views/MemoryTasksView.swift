import HarnessCore
import SwiftUI

/// Your asks and how far each has got, as agents keep them with `track`: in progress first,
/// then blocked, to do, and what was finished.
struct MemoryTasksView: View {
    @Environment(AppModel.self) private var app
    let library: MemoryLibrary

    private var tasks: [MemoryLibrary.Memory] {
        library.memories.filter { $0.isTask && $0.status == .active }
    }

    var body: some View {
        let tasks = tasks
        if tasks.isEmpty {
            ContentUnavailableView {
                Label("No Tasks Yet", systemImage: "checklist")
            } description: {
                Text("When you ask for something that takes several steps or more than one session, agents keep it here with its steps and where it stands, so any thread can pick it up where the last one left off.")
            }
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    ForEach([HarnessDatabase.TaskState.active, .blocked, .todo, .done], id: \.self) { state in
                        let group = tasks.filter { ($0.taskState ?? .todo) == state }.sorted { $0.updatedAt > $1.updatedAt }
                        if !group.isEmpty {
                            VStack(alignment: .leading, spacing: 8) {
                                HStack(spacing: 6) {
                                    Image(systemName: state.symbol).foregroundStyle(state.color)
                                    Text(state.label).font(.headline)
                                    Text("\(group.count)").foregroundStyle(.secondary)
                                }
                                LazyVGrid(columns: [GridItem(.adaptive(minimum: 300), spacing: 12)], spacing: 12) {
                                    ForEach(group) { task in
                                        TaskCard(task: task, library: library)
                                    }
                                }
                            }
                        }
                    }
                }
                .frame(maxWidth: 1000, alignment: .leading)
                .padding(24)
                .frame(maxWidth: .infinity)
            }
        }
    }
}

private struct TaskCard: View {
    @Environment(AppModel.self) private var app
    let task: MemoryLibrary.Memory
    let library: MemoryLibrary

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text(task.title).font(.body.weight(.semibold)).lineLimit(2)
                Spacer()
                Text(task.updatedAt, format: .relative(presentation: .named))
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            if !task.description.isEmpty {
                Text(task.description).font(.callout).foregroundStyle(.secondary).lineLimit(2)
            }
            if !task.steps.isEmpty {
                TaskProgressBar(task: task)
                if let next = task.nextStep, task.taskState != .done {
                    Label("Next: \(next)", systemImage: "arrow.turn.down.right")
                        .font(.callout)
                        .lineLimit(2)
                }
            }
            let threads = task.sources.compactMap { source in app.threads.first { $0.id.uuidString == source.threadID } }
            if !threads.isEmpty {
                HStack(spacing: 6) {
                    Image(systemName: "bubble.left.and.text.bubble.right").font(.caption).foregroundStyle(.secondary)
                    ForEach(threads.prefix(3)) { thread in
                        Button(thread.title) { app.selectedThreadID = thread.id }
                            .buttonStyle(.link)
                            .font(.caption)
                            .lineLimit(1)
                    }
                    if threads.count > 3 { Text("+\(threads.count - 3)").font(.caption).foregroundStyle(.secondary) }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .card()
        .contentShape(.rect)
        .onTapGesture {
            library.kindFilter = nil
            library.showsArchived = false
            library.selection = task.id
            library.tab = .memories
        }
        .help("Open the task")
    }
}

/// Done steps of all, as a bar and a count.
struct TaskProgressBar: View {
    let task: MemoryLibrary.Memory

    var body: some View {
        HStack(spacing: 8) {
            ProgressView(value: task.progress)
                .tint((task.taskState ?? .active).color)
            Text("\(task.stepsDone) of \(task.steps.count)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .fixedSize()
        }
    }
}

/// A task's state and steps, in its detail: tick steps, add them, move it along.
struct TaskSection: View {
    let task: MemoryLibrary.Memory
    let library: MemoryLibrary
    @State private var newStep = ""

    private var statePicker: some View {
        Picker("State", selection: Binding(get: { task.taskState ?? .active }, set: { state in change { task, value in task.taskState = value }(state) })) {
            ForEach(HarnessDatabase.TaskState.allCases, id: \.self) { Text($0.label).tag($0) }
        }
        .labelsHidden()
        .fixedSize()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                SectionLabel("Progress")
                Spacer()
                // A menu when there's no room for the segments.
                ViewThatFits(in: .horizontal) {
                    statePicker.pickerStyle(.segmented)
                    statePicker.pickerStyle(.menu)
                }
            }
            VStack(alignment: .leading, spacing: 0) {
                if !task.steps.isEmpty {
                    TaskProgressBar(task: task).padding(12)
                    Divider()
                }
                ForEach(Array(task.steps.enumerated()), id: \.offset) { index, step in
                    Toggle(isOn: Binding(get: { step.done }, set: { done in change { task, _ in task.steps[index].done = done }(()) })) {
                        Text(step.text).strikethrough(step.done, color: .secondary).foregroundStyle(step.done ? .secondary : .primary)
                    }
                    .toggleStyle(.checkbox)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                }
                HStack {
                    Image(systemName: "plus").foregroundStyle(.tertiary)
                    TextField("Add a step", text: $newStep)
                        .textFieldStyle(.plain)
                        .onSubmit {
                            let text = newStep.trimmingCharacters(in: .whitespacesAndNewlines)
                            guard !text.isEmpty else { return }
                            change { task, _ in task.steps.append(.init(text)) }(())
                            newStep = ""
                        }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .card()
        }
    }

    private func change<Value>(_ apply: @escaping (inout MemoryLibrary.Memory, Value) -> Void) -> (Value) -> Void {
        { value in
            var updated = task
            apply(&updated, value)
            library.save(updated)
        }
    }
}

/// The tasks a thread is working on, in its toolbar: where things stand at a glance.
struct ThreadTasksButton: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    @State private var showsTasks = false

    private var tasks: [HarnessDatabase.Memory] { thread.tasks }
    private var open: [HarnessDatabase.Memory] { tasks.filter { $0.taskState != .done } }

    var body: some View {
        Group {
            if let first = open.first ?? tasks.first {
                Button { showsTasks.toggle() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: (first.taskState ?? .active).symbol)
                            .foregroundStyle((first.taskState ?? .active).color)
                        Text(first.steps.isEmpty ? first.taskState?.label ?? "" : "\(first.stepsDone)/\(first.steps.count)")
                            .monospacedDigit()
                        if tasks.count > 1 { Text("+\(tasks.count - 1)").foregroundStyle(.secondary) }
                    }
                }
                .help("Tasks this thread is working on: \(tasks.map(\.title).joined(separator: ", "))")
                .popover(isPresented: $showsTasks, arrowEdge: .bottom) {
                    ThreadTasksPopover(thread: thread, tasks: tasks)
                }
            }
        }
    }

    /// Reads the thread's tasks again: on opening it and whenever the memory changes.
    static func load(_ thread: ThreadModel, app: AppModel) async {
        let database = app.store.database, id = thread.id.uuidString
        let tasks = await Task.detached { database.tasks(forThread: id) }.value
        if tasks != thread.tasks { thread.tasks = tasks }
    }
}

private struct ThreadTasksPopover: View {
    @Environment(AppModel.self) private var app
    let thread: ThreadModel
    let tasks: [HarnessDatabase.Memory]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(tasks) { task in
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text(task.title).font(.headline)
                        Spacer()
                        Chip(task.taskState?.label ?? "", tint: (task.taskState ?? .active).color)
                    }
                    if !task.steps.isEmpty {
                        TaskProgressBar(task: task)
                        ForEach(Array(task.steps.enumerated()), id: \.offset) { _, step in
                            Label(step.text, systemImage: step.done ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(step.done ? .secondary : .primary)
                                .font(.callout)
                        }
                    }
                }
            }
            Button("Show in Memory", systemImage: "brain") {
                guard let project = app.project(id: thread.projectID) else { return }
                app.showMemory(of: project)
                app.memoryLibrary(for: project).tab = .tasks
            }
            .buttonStyle(.link)
        }
        .padding(16)
        .frame(width: 340)
    }
}

extension HarnessDatabase.TaskState {
    var symbol: String {
        switch self {
        case .todo: "circle.dashed"
        case .active: "circle.lefthalf.filled"
        case .blocked: "exclamationmark.circle"
        case .done: "checkmark.circle.fill"
        }
    }

    var color: Color {
        switch self {
        case .todo: .secondary
        case .active: .accentColor
        case .blocked: .orange
        case .done: .green
        }
    }
}
