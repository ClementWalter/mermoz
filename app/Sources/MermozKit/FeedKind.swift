/// Shared card labels, symbols and colors give the Feed one visual vocabulary.
import Foundation

public enum FeedKind: String, Codable, Sendable, CaseIterable {
    case reply, decision, sign, promise, event, payment, intro, waiting, cleanup, reading, other

    /// The label at the top of the card.
    public var label: String {
        switch self {
        case .reply: "To answer"
        case .decision: "To decide"
        case .sign: "To sign"
        case .promise: "You promised"
        case .event: "Event"
        case .payment: "To pay"
        case .intro: "Someone new"
        case .waiting: "Waiting for an answer"
        case .cleanup: "Automatic mail"
        case .reading: "Newsletter"
        case .other: "To read"
        }
    }

    /// SF Symbol for the label.
    public var symbol: String {
        switch self {
        case .reply: "arrowshape.turn.up.left"
        case .decision: "arrow.triangle.branch"
        case .sign: "signature"
        case .promise: "checkmark.circle"
        case .event: "calendar"
        case .payment: "eurosign.circle"
        case .intro: "person.2"
        case .waiting: "clock"
        case .cleanup: "square.stack.3d.up"
        case .reading: "newspaper"
        case .other: "envelope"
        }
    }

    /// Hex color of the label, matching the app's status palette.
    public var color: String {
        switch self {
        case .reply: "#1F6FB5"
        case .decision: "#C2568C"
        case .sign: "#2E9A6B"
        case .promise, .payment: "#D9902F"
        case .event: "#C93A3A"
        case .intro: "#8A6FD1"
        case .waiting: "#2F8FCF"
        case .cleanup, .reading, .other: "#5A7386"
        }
    }
}
