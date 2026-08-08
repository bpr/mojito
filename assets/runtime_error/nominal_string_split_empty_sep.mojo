# expect: separator is empty
def main() raises:
    var parts = String("abc").split("")
    print(len(parts))
