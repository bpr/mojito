# PROBE (divergence): an iterator taken from a `List` element is staled by a
# store over that element.
#
# The pinned Mojo compiles this and prints the replaced element's codepoints
# (`q`, then two spaces): the iterator reads bytes the store destroyed. With a
# heap-sized string in place of `"q  "` it prints the freed string. Mojito
# rejects with "use of invalidated interior reference 'it' to
# 'ys["element"]~'", and rejects the same shape over a plain local
# (`var it = s.codepoints()` then `s = String("zz")`), which the pin also runs.
#
# Observed 2026-10-02 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Recorded in `docs/non-goals.md` ("A store over an element stales an iterator
# taken from it"). Promote to `assets/ownership_error/` when a re-pin rejects
# it.
def main():
    var ys: List[String] = [String("q  "), String("r ")]
    var it = ys[0].codepoints()
    ys[0] = String("zz")
    for c in it:
        print(String(c))
