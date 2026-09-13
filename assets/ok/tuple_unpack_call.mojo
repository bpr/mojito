# Unpacking a tuple returned from a function (evaluated once).
def pair() -> Tuple[Int, String]:
    return (1, String("one"))

def main():
    var a: Int = 0
    var b: String = String("")
    a, b = pair()
    print(a, b)
