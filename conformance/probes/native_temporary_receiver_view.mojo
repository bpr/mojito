# Question: a view-returning method called on a temporary owned receiver,
# iterated or measured in the same statement. The temporary lives to the end
# of the statement upstream, so the view stays valid: the pinned Mojo
# (`1.1.0.dev2026082605`) prints `a b c`, `c b a`, and `3`, and so does the
# Mojito VM.
#
# Mojito native today: traps with `use after Pointer deallocation` at the
# first loop. A named receiver (`var s = String("abc")`, then
# `s.__reversed__()`) and direct iteration (`for g in String("abc")`) both
# run natively.
#
# On the fix: move this program to `assets/ok` with its `exe-differential`
# manifest rows, and delete the matching native-backend entry in
# `docs/roadmap.md`.
def main():
    for g in String("abc").codepoints():
        print(g)
    for g in String("abc").__reversed__():
        print(g)
    print(len(String("abc").__reversed__()))
