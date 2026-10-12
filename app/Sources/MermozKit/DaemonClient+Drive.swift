/// The `drive.*` protocol methods (see docs/protocol.md › Google Drive).
import Foundation

extension DaemonClient {
    private func driveCall<R: Decodable>(_ method: String, _ params: Params, as _: R.Type = R.self) async throws -> R {
        let data = try await connection.call(method, params: params.data())
        return try RPCCoding.decoder().decode(R.self, from: data)
    }

    public func driveStatus() async throws -> [DriveStatus] { try await driveCall("drive.status", Params()) }

    /// Opens Google's consent for Drive; the result arrives as a `drive_changed` event.
    public func driveConnect(accountId: String) async throws -> AuthStart {
        try await driveCall("drive.connect", Params().with("account_id", accountId))
    }

    public func driveDisconnect(accountId: String) async throws {
        _ = try await connection.call("drive.disconnect", params: Params().with("account_id", accountId).data())
    }

    /// Linked, shared and saved Drive files of a thread's conversation.
    public func driveFiles(threadId: String) async throws -> DriveThreadFiles {
        try await driveCall("drive.files_for_thread", Params().with("thread_id", threadId))
    }

    /// Recently opened files without a query.
    public func driveSearch(accountId: String, query: String, limit: Int = 20) async throws -> [DriveFile] {
        try await driveCall("drive.search", Params().with("account_id", accountId).with("query", query).with("limit", limit),
                            as: DriveFileList.self).files
    }

    public func driveThumbnail(accountId: String, fileId: String) async throws -> DriveThumbnail? {
        try await driveCall("drive.thumbnail", Params().with("account_id", accountId).with("file_id", fileId))
    }

    public func driveSave(attachmentId: String) async throws -> SavedToDrive {
        try await driveCall("drive.save_attachment", Params().with("attachment_id", attachmentId))
    }

    /// Opens Google's file picker, optionally showing only `fileIds`.
    public func drivePick(accountId: String, fileIds: [String] = []) async throws -> DrivePickStart {
        var p = Params().with("account_id", accountId)
        if !fileIds.isEmpty { p = p.with("file_ids", fileIds) }
        return try await driveCall("drive.pick", p)
    }

    public func drivePickResult(_ pickId: String) async throws -> DrivePick {
        try await driveCall("drive.pick_result", Params().with("pick_id", pickId))
    }

    /// `role`: "reader" (view) or "commenter".
    public func driveShare(accountId: String, fileId: String, emails: [String], role: String) async throws -> DriveShareResult {
        try await driveCall("drive.share", Params().with("account_id", accountId).with("file_id", fileId)
            .with("emails", emails).with("role", role))
    }
}
