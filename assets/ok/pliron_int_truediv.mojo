# True division: `Float64 / Float64` divides, and the literal pair `1 / 4`
# divides exactly into a `FloatLiteral` the sum materializes to `Float64`.
def compute() -> Float64:
    var a = Float64(7)
    var b = Float64(-2)
    return a / b + 1 / 4

def main():
    print(compute())
