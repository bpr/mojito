# Nominal String keys flow through dict iteration, comprehension
# collection, and list append with independent copies.
def main() raises:
    var d: Dict[String, Int] = Dict[String, Int]()
    d["a"] = 1
    d["b"] = 2
    d["a"] = 10
    var total = 0
    for key in d:
        total += d[key]
    print(total)
    var keys = [key for key in d]
    print(len(keys))
