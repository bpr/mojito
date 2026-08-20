# expect: integer division or modulo by zero

def divisor(n: Int) -> Int:
    return n - n

def main():
    var x = 10 % divisor(3)
