# expect: type 'DType' has no field 'nosuch'
# `DType.<name>` must name a dtype.
def main():
    var d = DType.nosuch
    print(d)
