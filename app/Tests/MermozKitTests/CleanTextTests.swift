/// The Optimized view shows the AI's Markdown as formatted text.
import Foundation
import Testing
@testable import MermozKit

@Test(arguments: [
    ("**Objet** : devis", "Objet : devis"),
    ("- un\n- deux", "• un\n• deux"),
    ("Bonjour,\n\nMerci", "Bonjour,\n\nMerci"),
    ("voir [le lien](https://x.fr)", "voir le lien"),
])
func markdownBecomesReadableText(_ markdown: String, _ text: String) {
    #expect(String(CleanText.attributed(markdown).characters) == text)
}

@Test func boldIsStyled() {
    let s = CleanText.attributed("**Objet**")
    #expect(s.runs.first?.inlinePresentationIntent == .stronglyEmphasized)
}
