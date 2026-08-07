# expect: capturing
# A capturing closure no longer erases its environment into a plain
# `def(...)` list element: the element store rejects.
def main():
    var n = 1
    def bump(x: Int) unified {imm n} -> Int:
        return x + n
    var fns: List[def(Int) -> Int] = [bump]
    print(len(fns))
