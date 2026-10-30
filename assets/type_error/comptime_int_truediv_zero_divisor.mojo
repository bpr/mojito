# expect: integer division or modulo by zero
# A compile-time integer `/` by zero cannot be simplified: the pin rejects
# it ("could not simplify operator div(7, 0)"), where `//` masks it to 0.
def main():
    comptime x = Int(7)
    comptime y = x / 0
    print(y)
