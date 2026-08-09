# expect: has no field
# Field reads on a frozen struct fold at compile time and name real fields.
from layout import Layout

comptime L = Layout.row_major(2)
comptime BAD = L.missing

def main():
    print(BAD)
