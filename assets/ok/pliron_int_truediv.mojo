# True division. `Float64 / Float64` agrees with the pin; `Int / Int` does
# not — Mojito's is true division into `Float64`, the pin's truncates back to
# `Int` — so that pairing is the `int-true-division` conformance case.
def compute() -> Float64:
    var a = Float64(7)
    var b = Float64(-2)
    return a / b + 1 / 4

def main():
    print(compute())
