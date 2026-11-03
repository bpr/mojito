# A capturing closure's value is its storage struct, which does not convert
# to a run-time `def(...)` parameter; it binds an `F: def(...)` binder.
# expect: type mismatch for argument 'callback' to 'apply_twice'
def apply_twice(callback: def(Int) -> None):
    callback(2)
    callback(3)

def total() -> Int:
    var sum: Int = 0
    def add(value: Int) {mut sum}:
        sum += value
    apply_twice(add)
    return sum

def main():
    print(total())
