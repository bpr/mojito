# expect: Expected newline, ';', or EOF at the end of statement
# An `assert` takes one message expression, not a list, as at the pin.

def main():
    var n = 2
    assert n == 2, "n is ", n
