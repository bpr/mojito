# Calling a lambda inside a compile-time initializer is not supported yet.
# expect: not a compile-time value
def main():
    comptime whole = (lambda (x: Int) {} -> Int: x * 2)(21)
    print(whole)
