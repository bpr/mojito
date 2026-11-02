# A negative exponent of an integer `**` aborts in `_powi`, as at the pin
# since `1.3.0.dev2026110105`; it used to give 0 for a base other than ±1.

def exponent(n: Int) -> Int:
    return 0 - n

def main():
    var x = 2 ** exponent(3)
    print(x)
