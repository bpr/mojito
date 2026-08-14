# A Span element index outside `0..len` aborts (strict bounds; the arena
# check never gets a say).
# expect: abort: Span index out of range
def main():
    var xs: List[Int] = [1, 2, 3]
    var sp = Span(xs)
    print(sp[5])
