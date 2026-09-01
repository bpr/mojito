# `var b = a` on a Copyable-only List is an implicit copy: rejected.
def main():
    var a: List[Int] = [1, 2]
    var b = a
    print(len(b))
