# expect: 'InlineLevel' value has no attribute 'builtin'
@inline(.builtin)
def twice(x: Int) -> Int:
    return x * 2

def main():
    print(twice(21))
