# A negative exponent of an integer `**` does not trap: `_powi` gives 0 for
# a base other than ±1, as at the pin.

def exponent(n: Int) -> Int:
    return 0 - n

def main():
    var x = 2 ** exponent(3)
    print(x)
