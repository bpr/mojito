# PROBE (defect): a method called on a borrowed comprehension binder loses its
# receiver on the VM.
#
# The pinned Mojo prints `2`. Mojito checks the program but stops at run
# time: "vm backend does not support default/keyword/variadic arguments yet
# (call passed 0 args to 1-parameter function 'P.get')". The same call in a
# runtime `for` over the list runs, and so does `w.byte_length()` over a
# local `List[String]` in `main`; over a `List[String]` parameter it fails
# the same way.
#
# Observed 2026-09-25 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   2
#   mojito: run error: unsupported feature: vm backend does not support ...


@fieldwise_init
struct P(Copyable):
    var n: Int

    def get(self) -> Int:
        return self.n


def main():
    var ps: List[P] = [P(1), P(2)]
    var got = [item.get() for item in ps]
    print(got[1])
