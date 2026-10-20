# expect: undeclared
# A bare name in a destructuring target writes a variable in scope; it does
# not declare one.
def main():
    var b = 0
    (a, b) = (1, 2)
    print(a, b)
