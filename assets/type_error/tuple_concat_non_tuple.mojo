# expect: invalid call to 'concat'
# `Tuple.concat` takes another tuple, whose element pack it infers.
def main():
    var pair = Tuple(1, True)
    var joined = pair.concat(3)
    print(joined[0])
