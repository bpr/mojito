# A nested `def` may not name itself from its own body: both compilers reject
# the self-call and ask for the recursive helper at file scope.
def factorial(base: Int) -> Int:
    def fact(n: Int) {imm base} -> Int:
        if n <= 1:
            return base
        return n * fact(n - 1)
    return fact(5)

def main():
    print(factorial(1))
