# expect: expected ']' in call argument list
# `ref` is no type: a `List[ref T]` argument reads `ref` as a name and the
# referent as a stray token, as upstream parses it.
def stash(mut sink: List[ref Int]):
    pass

def main():
    print(1)
