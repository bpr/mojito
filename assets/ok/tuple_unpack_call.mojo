# Unpacking a tuple returned from a function (evaluated once).
def pair() -> Tuple[Int, StringLiteral]:
    return (1, "one")

def main():
    var a: Int = 0
    var b: StringLiteral = ""
    a, b = pair()
