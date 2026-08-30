# An origin placeholder in a function parameter annotation: the origin
# infers per call, exactly like the bare-generic parameter spelling.
def first(s: Span[Int, _]) -> Int:
    return s[0]

def main():
    var xs = List[Int]()
    xs.append(7)
    print(first(xs))
