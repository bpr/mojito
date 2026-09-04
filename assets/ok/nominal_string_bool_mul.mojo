# Boolable String (`Bool(s)` is non-emptiness) and `__mul__` repetition (a
# non-positive count is empty).
def describe(s: String) -> String:
    if Bool(s):
        return "non-empty"
    return "empty"

def main():
    print(Bool(String("")), Bool(String("x")))
    print(describe(String("")), describe(String("mojo")))
    var ab = String("ab")
    print(ab * 3, (ab * 0).byte_length(), (ab * -2).byte_length(), (ab * 1).byte_length())
    print(String("-") * 10)
    var repeated = ab * 2
    print(repeated, repeated * 2)
