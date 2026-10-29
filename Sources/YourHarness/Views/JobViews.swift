import SwiftUI

/// Above the composer while Claude has jobs running in the background: what each is, how
/// long it's been going and the last thing it printed, so it's clear whether Claude is
/// waiting on something or done. Claude is woken when one ends; you can type meanwhile.
struct JobsStrip: View {
    let thread: ThreadModel
    @State private var expanded: Set<String> = []

    var body: some View {
        let jobs = thread.liveJobs.filter(\.isBackground)
        VStack(alignment: .leading, spacing: 0) {
            Text(thread.isWaitingOnJobs ? "Claude is waiting on" : "Running in the background")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .padding(.top, 7)
            ForEach(jobs) { job in
                JobRow(job: job, expanded: expanded.contains(job.id),
                       toggle: { withAnimation(.snappy(duration: 0.2)) { expanded.formSymmetricDifference([job.id]) } },
                       stop: { thread.stopJob(job) })
            }
        }
        .padding(.bottom, 3)
        .card(radius: Theme.innerRadius + 4)
    }
}

private struct JobRow: View {
    let job: BackgroundJob
    let expanded: Bool
    let toggle: () -> Void
    let stop: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Button(action: toggle) {
                    HStack(spacing: 10) {
                        Image(systemName: job.isWatch ? "eye" : "terminal")
                            .foregroundStyle(.secondary)
                            .frame(width: 16)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(job.description.isEmpty ? "Command" : job.description)
                                .fontWeight(.medium)
                                .lineLimit(1)
                            if let line = job.lastLine, !line.isEmpty, !expanded {
                                Text(line)
                                    .font(.caption.monospaced())
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                        }
                        Spacer(minLength: 8)
                        Text(job.startedAt, style: .timer)
                            .font(.callout.monospacedDigit())
                            .foregroundStyle(.secondary)
                        Image(systemName: "chevron.right")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .rotationEffect(.degrees(expanded ? 90 : 0))
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .help(expanded ? "Hide its output" : "Show its output")
                Button("Stop", systemImage: "stop.fill", action: stop)
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                    .help(job.isWatch ? "Stop watching" : "Stop this command")
            }
            if expanded {
                JobOutput(job: job)
                    .transition(.opacity)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .modifier(FollowsJobOutput(job: job))
    }
}

/// The end of what a job printed, kept up while it runs.
struct JobOutput: View {
    let job: BackgroundJob
    @State private var text: String?

    var body: some View {
        CodeBox(text: text.map { $0.isEmpty ? "No output yet" : $0 } ?? (job.outputFile == nil ? "Its output shows once it reports" : "…"))
            .frame(maxHeight: 220)
            .task(id: job.isRunning) {
                guard let path = job.outputFile else { return }
                repeat {
                    if let tail = await BackgroundJob.tail(of: path) {
                        text = String(tail.split(separator: "\n", omittingEmptySubsequences: false).suffix(40).joined(separator: "\n"))
                    }
                    try? await Task.sleep(for: .seconds(1))
                } while job.isRunning && !Task.isCancelled
            }
    }
}

/// While a job runs and is on screen, its last line of output, once a second: for a watch,
/// what it last saw.
struct FollowsJobOutput: ViewModifier {
    let job: BackgroundJob

    func body(content: Content) -> some View {
        content.task(id: job.isRunning) {
            while job.isRunning, !Task.isCancelled {
                if let path = job.outputFile, let tail = await BackgroundJob.tail(of: path, bytes: 1_024) {
                    let line = tail.split(whereSeparator: \.isNewline).last.map(String.init)
                    if line != job.lastLine { job.lastLine = line }
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }
}

/// In a tool call's header: how long its job has run, or how it ended.
struct JobStatusLabel: View {
    let job: BackgroundJob

    var body: some View {
        HStack(spacing: 6) {
            if job.isRunning {
                Text(job.isBackground ? "In background" : "Running")
                Text(job.startedAt, style: .timer)
                    .monospacedDigit()
            } else {
                Text(summary)
            }
        }
        .font(.callout)
        .foregroundStyle(job.status == .failed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
        .lineLimit(1)
        .fixedSize()
    }

    private var summary: String {
        let took = job.endedAt.map { " · " + TurnDuration.format($0.timeIntervalSince(job.startedAt)) } ?? ""
        return switch job.status {
        case .running: ""
        case .completed: (job.isWatch ? "Ended" : job.exitCode.map { "Exited \($0)" } ?? "Done") + took
        case .failed: (job.exitCode.map { "Exited \($0)" } ?? "Failed") + took
        case .stopped: "Stopped" + took
        }
    }
}
