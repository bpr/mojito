# join writes each element between copies of the separator: a List argument
# converts to the Span parameter, any Writable element type works, and the
# String receiver itself is a Writer (`s.write(a, b, ...)`).
def main():
    var words: List[String] = ["alpha", "beta", "gamma"]
    print(String(", ").join(words))
    print(String("").join(words))
    var numbers: List[Int] = [1, 2, 3]
    print(String("-").join(numbers))
    var none: List[String] = []
    print(String(",").join(none).byte_length())
    var one: List[String] = ["solo"]
    print(String("+").join(one))
    var sink = String("")
    sink.write("x", 1, "y", 2.5, "z")
    print(sink)
    sink.write(String("!"))
    print(sink, sink.byte_length())
