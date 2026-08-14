# expect: String positional slicing was removed
# Positional String slicing (like bare positional `s[i]`) was removed in
# current Mojo: the unit is spelled explicitly through a keyword slice.
def main():
    var s: String = "hello"
    print(s[1:4])
