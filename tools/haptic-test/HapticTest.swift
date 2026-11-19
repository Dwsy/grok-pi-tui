import AppKit
import SwiftUI

@MainActor
final class HapticHost: ObservableObject {
    @Published var lastStatus = "点击下方按钮测试触控板反馈。"
    @Published var count = 0

    private var performer: NSHapticFeedbackPerformer? = NSHapticFeedbackManager.defaultPerformer

    func pulse(_ pattern: NSHapticFeedbackManager.FeedbackPattern, label: String) {
        let time: NSHapticFeedbackManager.PerformanceTime = .now
        performer?.perform(pattern, performanceTime: time)
        count += 1
        lastStatus = "第 \(count) 次：\(label)  已请求系统执行"
    }
}

struct ContentView: View {
    @StateObject private var host = HapticHost()

    var body: some View {
        VStack(spacing: 20) {
            Text("macOS 触控板反馈测试")
                .font(.title2.bold())
            Text(host.lastStatus)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(minHeight: 40)

            Group {
                Button("Alignment · 对齐") {
                    host.pulse(.alignment, label: "alignment")
                }
                Button("Level Change · 级别变化") {
                    host.pulse(.levelChange, label: "levelChange")
                }
                Button("Generic · 通用") {
                    host.pulse(.generic, label: "generic")
                }
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)

            Text("系统可能因无触控板或设置而忽略请求，因此“无感”不代表代码失败。")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .padding(32)
        .frame(width: 460, height: 320)
    }
}

@main
struct HapticTestApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}
