# `Int / Int` is true division into `Float64` in Mojito; upstream's truncates
# back to `Int` (only an `IntLiteral` pair divides into a float there).
def main():
    var a = 7
    var b = -2
    print(a / b)
