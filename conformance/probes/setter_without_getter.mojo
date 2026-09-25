# PROBE (divergence): a subscript store on a struct that declares
# `__setitem__` but no `__getitem__`.
#
# The pinned Mojo refuses the store ("'Sink' has '__setitem__' but no
# '__getitem__' method") and accepts the declaration while nothing
# subscripts it. Mojito runs the store through the setter.
#
# Observed 2026-09-24 against `Mojo 1.2.0.dev2026092105 (e9569894)`:
#   mojo:   rejects `s[0] = 3`
#   mojito: 3
#
# When fixed: move to `assets/type_error` with the pin's message.
struct Sink:
    var n: Int

    def __init__(out self):
        self.n = 0

    def __setitem__(mut self, i: Int, value: Int):
        self.n = value


def main():
    var s = Sink()
    s[0] = 3
    print(s.n)
