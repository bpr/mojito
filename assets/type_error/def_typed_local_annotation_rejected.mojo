# A bare `def(...)` local annotation names a trait in current Mojo, like the
# field and collection-element positions; only `def(...) thin` annotates a
# storable local callable value.
# expect: names a trait
def increment(x: Int) -> Int:
    return x + 1

def main():
    var callback: def(Int) -> Int = increment
    print(callback(41))
