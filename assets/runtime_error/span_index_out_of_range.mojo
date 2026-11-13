# A Span element index outside `0..len` aborts (strict bounds; the arena
# check never gets a say).
# expect: abort: index 5 is out of bounds, valid range is 0 to 2
def main():
    var xs: List[Int] = [1, 2, 3]
    var sp = Span(xs)
    print(sp[5])
