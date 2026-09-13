# Mojito truncates a `FloatLiteral` straight to `Int`; upstream makes you
# spell the `Float64` it truncates from.
def main():
    print(Int(3.9))
