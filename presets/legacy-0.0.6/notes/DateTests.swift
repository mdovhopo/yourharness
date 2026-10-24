import Foundation

let formatter = DateFormatter()
formatter.timeZone = TimeZone(identifier: "UTC")
formatter.dateFormat = "yyyy-MM-dd"
print(formatter.date(from: "2026-10-20") as Any)
