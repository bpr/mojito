# Keyword subscripts through the checked contract's keyword sources: index
# values and keyword slices on the nominal String, plus a keyword-only
# user-struct `__getitem__` whose actual binds by slot name — exercising
# kwarg slot reordering against the compiled contract. One borrowed view
# lives at a time: a held view conflicts with re-borrowing the source, and
# printing a view temporary directly trips a pre-existing VM
# temp-lifetime bug.
@fieldwise_init
struct Cells:
    var lo: Int
    var hi: Int

    def __getitem__(self, *, at: Int) -> Int:
        if at == 0:
            return self.lo
        return self.hi

def main() raises:
    var s = String("héllo")
    var first = s[byte=0:1]
    print(first, len(first))
    var second = s[codepoint=1:2]
    print(second, len(second))
    var pair = s[codepoint=0:2]
    print(pair)
    var c = Cells(3, 4)
    print(c[at=0], c[at=1])
