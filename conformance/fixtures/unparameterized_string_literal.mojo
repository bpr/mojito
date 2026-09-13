# Mojito's `StringLiteral` is one unparameterized value type, so it annotates
# variables and returns; upstream parameterizes it by the literal's own text,
# which leaves `StringLiteral` non-concrete everywhere but a parameter.
# StringLiteral as a value type: typed variables, parameter and return
# passing, copies, printing, and conversion into an owned String — all over
# the borrowed 16-byte descriptor.
def echo(s: StringLiteral) -> StringLiteral:
    return s


def pick(flag: Bool, a: StringLiteral, b: StringLiteral) -> StringLiteral:
    if flag:
        return a
    return b


def main():
    var s: StringLiteral = "typed literal storage"
    print(s)
    var t = s
    print(t)
    print(echo("round trip"))
    print(pick(True, "left", "right"))
    print(pick(False, "left", "right"))
    print(String(s))
    print("plain" == "plain")
