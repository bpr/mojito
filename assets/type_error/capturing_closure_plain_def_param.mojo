# A capturing closure cannot bind to an unqualified `def(...)` parameter in
# current Mojo; the contract must spell `capturing[...]`.
# expect: must spell 'capturing[...]'
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
