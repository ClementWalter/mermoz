/// What a drop carries into a merge, and the merge's undo token surviving the trip from
/// the daemon's answer back into `conversations.unmerge`.
import Foundation
import MermozKit
import Testing

@Test func aSelectionDropCarriesOneIdPerLine() {
    #expect(MergeDrop.ids(in: ["t1\nt2"]) == ["t1", "t2"])
}

@Test func aDraggedStatusCarriesNoThread() {
    #expect(MergeDrop.ids(in: ["status:todo"]).isEmpty)
}

@Test func anIdDroppedTwiceCountsOnce() {
    #expect(MergeDrop.ids(in: ["t1", "t1\nt2"]) == ["t1", "t2"])
}

@Test func aDropMergesIntoTheTargetLast() {
    #expect(MergeDrop.merge(["t1", "t2"], onto: "t0") == ["t1", "t2", "t0"])
}

@Test func aDropOnItselfMergesNothing() {
    #expect(MergeDrop.merge(["t1"], onto: "t1") == nil)
}

@Test func aSelectionDroppedOnOneOfItsOwnMergesNothing() {
    #expect(MergeDrop.merge(["t1", "t2"], onto: "t2") == nil)
}

@Test func anEmptyDropMergesNothing() {
    #expect(MergeDrop.merge([], onto: "t2") == nil)
}

private let answer = Data("""
{"conversation_id": "t0", "threads": 2, "members": ["t0", "t1"],
 "undo": {"threads": [{"id": "t1", "conversation": "t1", "state": "done"}],
          "pairs": [{"later": "t1", "earlier": "t0",
                     "prior": {"people": "[]", "jev": 0.4, "ai": null, "decision": "apart", "source": "ai", "created": 7}}]}}
""".utf8)

@Test func aMergeAnswerNamesItsMembers() throws {
    let r = try RPCCoding.decoder().decode(MergeResult.self, from: answer)
    #expect(r.members == ["t0", "t1"])
}

@Test func theUndoTokenGoesBackAsTheDaemonWroteIt() throws {
    let r = try RPCCoding.decoder().decode(MergeResult.self, from: answer)
    let sent = try JSONSerialization.jsonObject(with: Params().with("undo", r.undo).data()) as? [String: Any]
    let written = try JSONSerialization.jsonObject(with: answer) as? [String: Any]
    let back = NSDictionary(dictionary: sent?["undo"] as? [String: Any] ?? [:])
    let undo = (written?["undo"] as? [String: Any] ?? [:]).withoutNulls
    #expect(back.isEqual(to: undo))
}

private extension Dictionary where Key == String, Value == Any {
    /// JSON nulls dropped at every depth: Swift leaves a nil optional out, which serde
    /// reads back as None all the same.
    var withoutNulls: [String: Any] {
        func clean(_ v: Any) -> Any? {
            if v is NSNull { return nil }
            if let d = v as? [String: Any] { return d.compactMapValues(clean) }
            if let a = v as? [Any] { return a.compactMap(clean) }
            return v
        }
        return compactMapValues(clean)
    }
}
