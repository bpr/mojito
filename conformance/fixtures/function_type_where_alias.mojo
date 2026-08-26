# mojo-only (strict-subset gap): the audited head runs the changelog's
# `comptime Kernel = def[w: Int](Int) thin -> None where (...)` alias
# spelling (prints 7). Mojito rejects it with the PRE-EXISTING inability to
# alias any function type via `comptime` ("not a compile-time value:
# unsupported compile-time type argument") — the where clause itself is not
# the blocker; the inline-bound spelling of the same contract runs
# (assets/ok/function_type_where_bound.mojo).
comptime Kernel = def[w: Int](Int) thin -> None where (w > 0, "width must be positive")

def kernel[w: Int](x: Int) where (w > 0, "width must be positive"):
    print(w + x)

def apply[F: Kernel](x: Int):
    F[4](x)

def main():
    apply[kernel](3)
