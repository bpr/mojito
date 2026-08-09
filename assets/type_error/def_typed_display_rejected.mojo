# An uncontextualized display of function values would infer a callable
# element type; current Mojo has no callable-value element storage.
# expect: collection display element
def double(x: Int) -> Int:
    return x * 2

def main():
    var fns = [double]
    print(len(fns))
