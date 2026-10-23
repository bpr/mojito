# PROBE (re-probe): does a `try` nested in a `finally` copied before two
# exits compile?
#
# **Differs.** Found while landing R884 (2026-10-22) against Mojo
# 1.2.0.dev2026101705 (46f49943). The outer `finally` is pasted before the
# `continue` and at the `else`'s `try.yield`; with a `try`/`except`/
# `finally` inside it, the pin rejects the program, "use of uninitialized
# value '(expression temporary)'" at `print("inner fin", s.tag)`, then
# "failed to run the pass manager", the error
# `finally_copy_comptime_for.mojo` meets. Mojito runs it, printing
# `body 0`, `inner fin 0`, `drop 0`, `inner caught g1 1`, `inner fin 1`,
# `drop 1`, `body 2`, `inner caught g2 2`, `inner fin 2`, `drop 2`, `end`.
# When the pin runs it, promote it to `assets/ok/`.
#
# Run:    mojo run finally_copy_nested_try.mojo
#         cargo run -- run conformance/probes/finally_copy_nested_try.mojo


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def g(x: Int) raises:
    if x > 0:
        raise Error("g" + String(x))


def main():
    for i in range(3):
        var s = S(i)
        try:
            if i == 1:
                continue
            print("body", i)
        finally:
            try:
                g(i)
            except e:
                print("inner caught", e, s.tag)
            finally:
                print("inner fin", s.tag)
    print("end")
