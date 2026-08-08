# Negative-step slicing reverses the nominal String's bytes, like the
# builtin literal slice.
def main():
    var s: String = "hello"
    print(s[::-1])
    print(s[::-1] == String("olleh"))
    print(s[3:0:-1] == String("lle"))
