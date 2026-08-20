# expect: '**' exponent must be a non-negative Int

def exponent(n: Int) -> Int:
    return 0 - n

def main():
    var x = 2 ** exponent(3)
