# StringLiteral as a value type: typed parameters, copies, printing, and
# conversion into an owned String — all over the borrowed 16-byte descriptor.
# A *runtime* StringLiteral value (a variable or return annotated
# `StringLiteral`, or one picked by a runtime flag) is Mojito-only: upstream
# parameterizes the type by the literal itself, so only the parameter position
# infers. See the `unparameterized-string-literal` conformance case.
def echo(s: StringLiteral) -> String:
    return String(s)


def pick(flag: Bool, a: StringLiteral, b: StringLiteral) -> String:
    if flag:
        return String(a)
    return String(b)


def main():
    var s = "typed literal storage"
    print(s)
    var t = s
    print(t)
    print(echo("round trip"))
    print(pick(True, "left", "right"))
    print(pick(False, "left", "right"))
    print(String(s))
    print("plain" == "plain")
