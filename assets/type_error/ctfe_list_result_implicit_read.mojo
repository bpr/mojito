# expect: List[Int]
def make() -> List[Int]:
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    return xs^

def main():
    comptime xs = make()
    print(xs[0])
