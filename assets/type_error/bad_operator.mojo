# Adding an Int and a Bool is a type error.
# expect: operator '+'
def main():
    var x: Int = 1 + True
    print(x)
