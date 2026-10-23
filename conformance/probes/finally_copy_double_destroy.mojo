# PROBE (re-probe): is a value read in a `finally` copied before a `break`
# out of an inner `while True` destroyed once?
#
# **Differs.** Found while landing R884 (2026-10-22) against Mojo
# 1.2.0.dev2026101705 (46f49943). The pin destroys `s` twice per outer
# iteration: `drop 0` before the first `in fin 1 0`, which still reads it,
# and `drop 0` again after `in fin 2 0`. Mojito destroys it once, after its
# last read in the inner `finally`'s copy: `in fin 1 0`, `in fin 2 0`,
# `drop 0`, `out fin 0`, then the same for `1`, then `end`. `S` is not
# copyable, so the pin's two destructions are of one value. When the pin
# destroys it once, promote this file to `assets/ok/`.
#
# Run:    mojo run finally_copy_double_destroy.mojo
#         cargo run -- run conformance/probes/finally_copy_double_destroy.mojo


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def main():
    var outer = 0
    while outer < 2:
        var s = S(outer)
        try:
            var k = 0
            while True:
                try:
                    k += 1
                    if k == 2:
                        break
                finally:
                    print("in fin", k, s.tag)
            if outer == 1:
                break
        finally:
            print("out fin", outer)
        outer += 1
    print("end")
