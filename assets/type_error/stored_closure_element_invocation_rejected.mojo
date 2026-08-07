# expect: not callable
# A closure value can reach a collection element through a list literal, but
# element invocation is not offered — the stored value is inert.
def main():
    var n = 1
    def bump(x: Int) unified {imm n} -> Int:
        return x + n
    var fns: List[def(Int) -> Int] = [bump]
    print(fns[0](1))
