# Pin divergence probe (Mojo 1.2.0.dev2026092105): an `Int`-annotated local
# `comptime` binding whose value is a `Bool`. The pin rejects it with
# "cannot implicitly convert 'Bool' value to 'Int'"; Mojito accepts it and
# prints `True`, keeping the value a `Bool`. A template body's binding over
# its binders is rejected as at the pin. Roadmap R485.
def main():
    comptime m: Int = 1 > 0
    print(m)
