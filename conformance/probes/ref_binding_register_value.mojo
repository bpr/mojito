# Question: a `ref` binding to a register-passable value — here a SIMD
# lane read, which `SIMD.__getitem__` returns by value. Upstream
# (`1.1.0.dev2026082605`) rejects it: `value of type 'Int32' doesn't have
# a memory origin in 'ref' binding`. It rejects `ref y = f()` for an
# `Int`-returning `f` and `ref y = a + 1.0` the same way, while a
# memory-backed temporary (`ref x = make_list()`) is accepted by both.
#
# Mojito today: accepts and prints `[1, 4, 3, 4]` — the binding writes
# through to the vector's lane.
#
# On the fix: make the checker's `ref` binding reject a register-passable
# value with upstream's text and move this program to `assets/type_error`.
# Then delete the `ref-binding-register-value` bullet from the
# behavioral-divergences list in `docs/roadmap.md`.
def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var i = 1
    ref lane = v[i]
    lane += 2
    print(v)
