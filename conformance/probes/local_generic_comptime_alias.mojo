# Pin gap probe (Mojo 1.2.0.dev2026092105): a parametric `comptime` alias
# declared in a function body. The pin prints `11`. Mojito rejects it with
# "a generic comptime alias must be declared at module scope". Roadmap R502.
def f():
    comptime A[k: Int] = k + 1
    print(A[10])


def main():
    f()
