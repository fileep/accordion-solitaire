#!/bin/bash
python falsify.py skip > falsify2.log 2>&1
python anatomy.py 1005616798 112249515 > anatomy3.log 2>&1
python dawg.py 1e9 8e6 > all23.log 2>&1
echo done > chain.done
