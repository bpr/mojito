# expect: String slicing is contiguous
def main() raises:
    var s: String = "hello"
    print(s[::-1])
