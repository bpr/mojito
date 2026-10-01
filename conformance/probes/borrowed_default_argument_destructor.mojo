# PROBE: is an evaluated default handed to a borrowing parameter destroyed?
#
# **Differs.** The pin prints `use dflt`, `drop dflt`, `1`: the default
# value is destroyed when the call returns, as an explicitly passed
# temporary is. Mojito's VM and native backend print `use dflt`, `1` and
# never run the destructor. A `var` parameter's default is destroyed by the
# callee on both sides. Filed in `docs/roadmap.md` §3. When `run` prints
# `drop dflt`, fold this case into
# `assets/ok/evaluated_default_argument.mojo`.
#
# Observed 2026-09-30 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run borrowed_default_argument_destructor.mojo


struct R:
    var tag: String

    def __init__(out self, tag: String):
        self.tag = tag

    def __deinit__(deinit self):
        print("drop", self.tag)


def use(r: R = R(String("dflt"))) -> Int:
    print("use", r.tag)
    return 1


def main():
    print(use())
