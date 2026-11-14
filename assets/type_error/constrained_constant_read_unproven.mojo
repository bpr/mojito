# expect: cannot implicitly convert '__generator_type Int where False' value to 'Int'
# A read instantiates a constrained constant at its declared type, which
# needs its `where` clause proven.
comptime Never: Int where (1 > 5, "never") = 1


def main():
    var x: Int = Never
    print(x)
