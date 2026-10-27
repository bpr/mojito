# A closed local `comptime` value binding is the check's parameter
# expression, as every local `comptime` is at the pin: a conditional over
# closed bindings, a sized-scalar construction, a local string constant in a
# bracket, and a generic alias that applies a function.

comptime KGENString = __mlir_type.`!kgen.string`


def twice(n: Int) -> Int:
    return n * 2


comptime Twice[n: Int] = twice(n)


def bind[s: StringLiteral]():
    print(s)


def kbind[s: KGENString](label: String):
    print(label)


def main():
    comptime t = (5, 1)
    comptime c = t[0] == 1
    comptime x = t[1] if c else t[0]
    print(x)
    comptime d = UInt8(250)
    print(d)
    comptime e = UInt8(250) + 10
    print(e)
    comptime local = "Lo"
    bind[local]()
    kbind[local.value]("local")
    comptime z = Twice[6]
    print(z)
