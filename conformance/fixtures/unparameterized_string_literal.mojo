# `StringLiteral` is parameterized by the literal's own text, so the bare
# spelling is concrete only as a parameter annotation: a return and a variable
# annotated `StringLiteral` are rejected (`'StringLiteral[_]' is not concrete`).
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
