# The stdlib dict key/value iterators delegate stepping to the wrapped entry
# iterator with upstream's expression-origin ref returns; iteration through
# keys() and values() observes insertion order and live values.
def main():
    var d = Dict[String, Int]()
    d["one"] = 1
    d["two"] = 2
    d["three"] = 3
    for k in d.keys():
        print(k)
    for v in d.values():
        print(v)
