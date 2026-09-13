# Nested `def`s (closures) are lifted to functions whose captured enclosing locals
# carry explicit immutable or mutable environments, so reads, writes, and calls
# to top-level functions execute through the VM. A nested `def` may not call
# itself — the pin rejects that, and the `recursive-nested-def` conformance case
# carries the divergence — so the recursion here lives at file scope.
def double(x: Int) -> Int:
    return x * 2

def fact(n: Int, base: Int) -> Int:
    if n <= 1:
        return base
    return n * fact(n - 1, base)

def adder(n: Int) -> Int:
    def add_n(x: Int) {imm n} -> Int:
        return double(x) + n
    return add_n(100)

def counter() -> Int:
    var total: Int = 0
    def add(x: Int) {mut total}:
        total = total + x
    add(5)
    add(3)
    return total

def factorial(base: Int) -> Int:
    def call(n: Int) {imm base} -> Int:
        return fact(n, base)
    return call(5)

def main():
    print(adder(21))
    print(counter())
    print(factorial(1))
