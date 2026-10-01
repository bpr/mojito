# PROBE: a nested `def` reading the enclosing function's `comptime` binding.
#
# **Differs.** The pin prints `4`. Mojito rejects the program with
# "Could not infer capture convention of the captured value n". Filed in
# `docs/roadmap.md` §3. When `run` prints `4`, promote this file to
# `assets/ok/` with its manifest rows.
def main():
    comptime n = 3

    def inner() -> Int:
        return n + 1

    print(inner())
