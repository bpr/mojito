# Ordinary String, StringSpan, and StringLiteral iteration yields borrowed
# grapheme-cluster StringSpan views (current Mojo): one view per extended
# grapheme cluster under the documented UAX #29 subset, including the
# ZWJ-joined and regional-indicator clusters the grapheme indexing fixture
# pins.
def main():
    var s = String("héllo🙂")
    var pieces = 0
    var bytes = 0
    for g in s:
        pieces += 1
        bytes += len(g)
    print(pieces, bytes)
    var family = String("👨‍👩‍👧")
    var sp = StringSpan(family)
    var clusters = 0
    for g in sp:
        clusters += 1
        print(len(g))
    print(clusters)
    for g in "ab":
        print(g)
