# Walrus evaluation occurs before the later runtime failure.
# expect: boom
def main() raises:
    var n: Int = 0
    var first: Int = (n := 5)
    raise Error("boom")
