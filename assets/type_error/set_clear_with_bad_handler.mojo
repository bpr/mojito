# expect: no overload matches
def main():
    var s: Set[Int] = {1}
    s.clear_with(lambda (a: Int, b: Int): None)
