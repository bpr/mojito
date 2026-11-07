# expect: Unexpected token Assert
# `assert` is a reserved word at the pin, so it names no variable.

def main():
    var assert = 1
    print(assert)
