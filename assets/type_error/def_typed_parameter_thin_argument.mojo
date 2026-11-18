# A non-`thin` `def(...)` names a trait, so a run-time parameter of that type
# takes no value: a thin function does not convert to it.
# expect: type mismatch for argument 'f' to 'via'
def via(f: def(Int) -> Int, x: Int) -> Int:
    return x

def plus(x: Int) -> Int:
    return x + 1

def main():
    print(via(plus, 1))
