# An immediately invoked lambda evaluates inside a compile-time initializer.
def main():
    comptime whole = (lambda (x: Int) {} -> Int: x * 2)(21)
    print(whole)
