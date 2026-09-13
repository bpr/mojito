# Mojito lifts a nested `def` that calls itself, carrying its captured
# environment through the recursion; the pin rejects a self-reference from a
# nested function outright and tells you to define it at file scope.
def factorial(base: Int) -> Int:
    def fact(n: Int) {imm base} -> Int:
        if n <= 1:
            return base
        return n * fact(n - 1)
    return fact(5)

def main():
    print(factorial(1))
