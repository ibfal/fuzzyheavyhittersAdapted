# MOSAIC

This is a Rust implementation of the Mosaic framework in the paper _Mosaic: A Modular Framework for Private Fuzzy Heavy Hitters_ (CCS 2026).
The description of the problem setting for Mosaic can be found in the [Problem Settings](#problem-settings) section, and we describe our solution in the [Mosaic's Solution](#mosaics-solution) section.
The details about the artifact, including [Installation](#installation) and [How to run](#how-to-run) is shown afterward.

# Problem Settings

## What problem does Mosaic solve?
We consider the _Private Fuzzy Heavy Hitters_ problem. 
Given a dataset, a _Heavy Hitter_ could be simply thought of as a _popular_ item in this set.
For example, if this is a dataset of emojis, we want to know which emoji is the most liked by users.
In some other datasets, such as for ride-sharing applications, the locations stored are _not exact_, hence we consider the _Fuzzy Heavy Hitters_ problem for this use case.

More formally, given a dataset of $d$-dimensional points, a _Fuzzy Heavy Hitter_ is defined based on the following three parameters:
- The distance radius $\delta$, which we confine to be only positive integer,
- The distance metric $d$, which is either $L_\infty$ or $L_p$, and
- The count threshold $t$.

The question that whether a point $x$ is a Fuzzy Heavy Hitter is equivalent to the following question:
**Does there exists at least $t$ other points $y$ in the dataset, such that $d(x, y) \le \delta$?**

## Private Fuzzy Heavy Hitters
The definition of _Fuzzy Heavy Hitters_ already half of the story, the other half is: What does _private_ mean in this case?
First, we have $n$ clients, each will provide a $d$-dimensional point e.g. his/her starting point of the ride.
Now, a data analyst only wants to learn the Fuzzy Heavy Hitters. 
He does not want to know each client's provided point, and vice versa, the client also does not want the point to be leaked to the analyst.

In more details, there are two problem settings for Private Fuzzy Heavy Hitters that we consider in Mosaic:
- **Known Dictionary**: The data analyst has a list of potential Fuzzy Heavy Hitters, and wants to find out which ones of them are actually popular.
- **Unknown Dictionary**: The data analyst does not have any information beforehand, and discovers all the heavy hitters from the clients' dataset.

## Who are the parties in Mosaic?
In Mosaic, we realize this functionality with the help from two servers, which will "collect" the points from clients in a private way, then do some aggregation to learn only the final Fuzzy Heavy Hitters.
We illustrate the protocol by implementing the `server`, the `client`, and the `dealer`. 
All CLI codes can be found in **src/bin/mosaic_end_to_end**.
- `server0` and `server1`: The source code for the servers' CLI is in **mosaic_server.rs**. At the start of the protocol, the servers receive _secret sharings_ (or rather _function secret sharings_) of all clients' points. 
Then two servers run a distributed protocol that either check the heavy hitters (_known dictionary_ setting) or discover the heavy hitters (_unknown dictionary_ setting).
- `client`: We use a single file to simulate and generate _function secret sharing_ for many client points, instead of spawning out one instance for each client. 
The CLI's source code can be found in **mosaic_client.rs**.
The clients goes offline after sending the secret shared points to the servers.
- `dealer`: There are in fact two options to run the distributed protocol between the servers, with one including the help from a dealer. 
The dealer's task is to generate _correlated randomness_ that does not require any information about the actual protocol data.
This means the random correlations can be generated _continuously_, or _processing for life_, and can be sent to the servers upon request.
The CLI code for the dealer is in **mosaic_dealer.rs**.

For more details about the math, please refer to the paper.

## Threat Model
In Mosaic, the model we follow is:
- The two servers are _non-colluding_ e.g. two different non-profit organizations helping with this task.
- The clients cannot learn any other point, but can try to provide _malformed_ sharings of points to mess up the computation.

# The modular design of Mosaic
To find out whether a point is a Fuzzy Heavy Hitter (privately), Mosaic divides the problem into three steps:

1. **PB-iFSS**: This fancy name just means, each client provides a _Function Secret Sharing_ for their point. 
The function that the clients need to share will determine the _distance_ between a specific other point and the client's point.
The two servers can _evaluate_ the current point using these function secret shares to obtain the distance in a distributed way, before proceeding to the next step. 
We provide $2$ ways to do PB-iFSS: one way using _Oblivous Key-Value Store_ (implemented in `src/okvs_f2k.rs`), and the other way uses the GGM-based _Function Secret Sharing_ paradigm (implemented in `src/fss`).
The code for the sharing phase is implemented in `src/fuzzy_match/share_phase.rs`.
2. **Distance checking**: After obtaining the secret shares of the distance, the next step in Mosaic is to compare this distance with the radius $\delta$.
We can either realize this distributed process via vanilla 2PC (in this implementation, using _Garbled Circuits_, implemented in `src/garbled_circuits`), or with the help of a dealer (who will use, again, Function Secret Sharing).
The source code for this procedure is in `src/fuzzy_match/check_phase.rs`.
3. **Threshold comparison**: In the last step, for each pair of points $(x, y)$, the distance checking procedure will output a $0-1$ result. 
Now for the point of interest, the servers simply aggregate all the comparison results, and compare this aggregation (still in the secret-share form) with a threshold $t$.
Again, there are two ways to implement this secure comparison, the source code can be found in `src/fuzzy_match/threshold_phase.rs`.

The above are all the components to consider _one point_. 
For the _known dictionary_ case, the servers simply iterate through the whole dictionary, while for the _unknown dictionary_ case, the servers need to go through a binary tree traversal algorithm to discover the fuzzy heavy hitters.
The source code for the protocol (both cases) is in `src/fuzzy_match/protocol.rs`.


# Experimental Results

## Machine and dataset
All experiments are run on AWS EC2 instances. 
We simulate the clients with a **t2.2xlarge** instance, which means the computation power for the clients is also weak. 
The two servers and the dealer, each is simulated with a **c5a.24xlarge** instance. 
Each run from the servers and the dealer is parallelized through $32$ cores.
Currently, we only have experiments in the LAN setting.

In our experiments, we clean the RideAustin dataset and obtain three sub-datasets with different sizes, in order to test the scalability of Mosaic. 
Each sub-dataset is stored in a `.csv`, that is available in the `data` folder in the Zenodo repository of Mosaic. 
For example, the csv file for the rides in the busiest day (the other two datasets are for the busiest week and the busiest month) can be found at `data/sample_busiest_day.csv` on Zenodo.
The summary of the size of each sub-dataset is shown in the following table.
We also report here the number of Fuzzy Heavy Hitters discovered in our experiments for each distance metric. 
Intuitively, more heavy hitters found means it also takes more time for the experiment to run. 
These number would be consistent with the running time we report in the next subsections.
The users can confirm these numbers by running the ground-truth program (with more details in the [Ground truth](#running-the-ground-truth) section).
<div align="center">

| Dataset | busiest_day | busiest_week | busiest_month |
| --- | ---: | ---: | ---:|
| Number of client points | $21,581$ | $59,040$ | $115,174$ |
| $2\%$ match threshold | $431$ | $1,180$ | $2,303$ |
| $L_\infty$ fuzzy heavy hitters | 916 | 1,100 | 1,103 |
| $L_1$ fuzzy heavy hitters | 426 | 468 | 444 |
| $L_2$ fuzzy heavy hitters | 598 | 643 | 652 |

</div>


## Small-scale runs for unknown dictionary
We first run Mosaic with every possible combination of methods in client sharing, fuzzy match, and threshold comparison. 
In total, the number of combinations is $2\times2\times2 = 8$ options per distance metric. 
We run the experiments for $3$ different distance metrics: $L_{\infty}, L_1, L_2$.
The experiments are run on the **busiest_day** sub-dataset. 
Based on the results of the small-scale runs, we choose some options that take the smallest amount of time to run for the larger sub-datasets.
Some analyses of the experimental results are as follow:
- **Efficiency**: Mosaic only takes half a minute to return the Fuzzy Heavy Hitters for the **busiest_day** dataset, in the $L_\infty$ distance metric, while preserving privacy for the clients. 
In other runs, most end within $3$ minutes, and the slowest option ends within $10$ minutes
- **Lightweight client work**: Each client only needs to spend less than $\frac{3}{20000}$ second to generate the key, and send less than $\frac{1}{100}$ MB of data, which means the data cost for client is small.
- **Garbled Circuits is faster for LAN**: The gc_gc method is consistently the fastest protocol in LAN, with running time being correspondingly $32.4$ seconds, $71.5$ seconds, and $92.3$ seconds (all less than $1$ minute and a half) for $L_\infty$, $L_1$, and $L_2$.
The dealer-based approach with FSS is not without merit, as the amount of data being read by each server is more balanced, and is smaller than the GC approach. 
This means for the scenario where the dealer is much stronger in compute with better internet bandwidth, the FSS approach would be more suitable.

<div align="center">

<table>
  <tr>
    <th>Distance Metric</th>
    <th>FSS</th>
    <th>Method</th>
    <th>total client send</th>
    <th>total client time</th>
    <th>server0 send</th>
    <th>server1 send</th>
    <th>dealer send to each server</th>
    <th>time</th>
  </tr>
  <tr>
    <td rowspan="8"><span style="font-style: italic;">L</span><sub>&infin;</sub></td>
    <td rowspan="4">FSS</td>
    <td>gc_gc</td>
    <td rowspan="4">192 MB</td>
    <td rowspan="4">2.91s</td>
    <td>2.74 GB</td>
    <td>11.6 GB</td>
    <td>0</td>
    <td>32.4 s</td>
  </tr>
  <tr>
    <td>gc_fss</td>
    <td>2.61 GB</td>
    <td>11.1 GB</td>
    <td>3.60 MB</td>
    <td>31.8 s</td>
  </tr>
  <tr>
    <td>fss_gc</td>
    <td>106 MB</td>
    <td>111 MB</td>
    <td>6.55 GB</td>
    <td>33.7 s</td>
  </tr>
  <tr>
    <td>fss_fss</td>
    <td>104 MB</td>
    <td>104 MB</td>
    <td>6.56 GB</td>
    <td>34.2 s</td>
  </tr>
  <tr>
    <td rowspan="4">OKVS</td>
    <td>gc_gc</td>
    <td rowspan="4">87.0 MB</td>
    <td rowspan="4">6.24 s</td>
    <td>98.5 GB</td>
    <td>490 GB</td>
    <td>0</td>
    <td>286 s</td>
  </tr>
  <tr>
    <td>gc_fss</td>
    <td>98.5 GB</td>
    <td>490 GB</td>
    <td>3.60 MB</td>
    <td>302 s</td>
  </tr>
  <tr>
    <td>fss_gc</td>
    <td>779 MB</td>
    <td>784 MB</td>
    <td>301 GB</td>
    <td>631 s</td>
  </tr>
  <tr>
    <td>fss_fss</td>
    <td>776 MB</td>
    <td>776 MB</td>
    <td>302 GB</td>
    <td>685 s</td>
  </tr>
  <tr>
    <td rowspan="8"><span style="font-style: italic;">L</span><sub>1</sub></td>
    <td rowspan="4">FSS</td>
    <td>gc_gc</td>
    <td rowspan="4">511 MB</td>
    <td rowspan="4">6.66 s</td>
    <td>2.95 GB</td>
    <td>26.7 GB</td>
    <td>0</td>
    <td>71.5 s</td>
  </tr>
  <tr>
    <td>gc_fss</td>
    <td>2.95 GB</td>
    <td>26.7 GB</td>
    <td>2.25 MB</td>
    <td>72.6 s</td>
  </tr>
  <tr>
    <td>fss_gc</td>
    <td>727 MB</td>
    <td>730 MB</td>
    <td>15.8 GB</td>
    <td>84.0 s</td>
  </tr>
  <tr>
    <td>fss_fss</td>
    <td>725 MB</td>
    <td>725 MB</td>
    <td>15.8 GB</td>
    <td>88.7 s</td>
  </tr>
  <tr>
    <td rowspan="4">OKVS</td>
    <td>gc_gc</td>
    <td rowspan="4">85.1 MB</td>
    <td rowspan="4">6.30 s</td>
    <td>32.6 GB</td>
    <td>265 GB</td>
    <td>0</td>
    <td>165 s</td>
  </tr>
  <tr>
    <td>gc_fss</td>
    <td>32.6 GB</td>
    <td>265 GB</td>
    <td>2.25 MB</td>
    <td>170 s</td>
  </tr>
  <tr>
    <td>fss_gc</td>
    <td>555 MB</td>
    <td>558 MB</td>
    <td>199 GB</td>
    <td>439 s</td>
  </tr>
  <tr>
    <td>fss_fss</td>
    <td>555 MB</td>
    <td>558 MB</td>
    <td>199 GB</td>
    <td>439 s</td>
  </tr>
  <tr>
    <td rowspan="8"><span style="font-style: italic;">L</span><sub>2</sub></td>
    <td rowspan="4">FSS</td>
    <td>gc_gc</td>
    <td rowspan="4">691 MB</td>
    <td rowspan="4">8.19 s</td>
    <td>4.88 GB</td>
    <td>42.4 GB</td>
    <td>0</td>
    <td>92.3 s</td>
  </tr>
  <tr>
    <td>gc_fss</td>
    <td>4.87 GB</td>
    <td>42.4 GB</td>
    <td>2.67 MB</td>
    <td>91.3 s</td>
  </tr>
  <tr>
    <td>fss_gc</td>
    <td>916 MB</td>
    <td>919 MB</td>
    <td>26.9 GB</td>
    <td>117 s</td>
  </tr>
  <tr>
    <td>fss_fss</td>
    <td>913 MB</td>
    <td>913 MB</td>
    <td>26.9 GB</td>
    <td>126 s</td>
  </tr>
  <tr>
    <td rowspan="4">OKVS</td>
    <td>gc_gc</td>
    <td rowspan="4">85.1 MB</td>
    <td rowspan="4">6.32 s</td>
    <td>40.1 GB</td>
    <td>326 GB</td>
    <td>0</td>
    <td>196 s</td>
  </tr>
  <tr>
    <td>gc_fss</td>
    <td>40.1 GB</td>
    <td>326 GB</td>
    <td>2.67 MB</td>
    <td>200 s</td>
  </tr>
  <tr>
    <td>fss_gc</td>
    <td>660 MB</td>
    <td>664 MB</td>
    <td>244 GB</td>
    <td>536 s</td>
  </tr>
  <tr>
    <td>fss_fss</td>
    <td>658 MB</td>
    <td>658 MB</td>
    <td>244 GB</td>
    <td>603 s</td>
  </tr>
</table>

</div>

_Remarks_: The running time reported here is much faster than what was reported in Table 4 in our submission, due to the fact that the authors forgot to parallelize the FSS evaluation part in the unknown dictionary setting, and more optimizations since the time of submission. Please refer to the [Optimizations](#optimizations) section for more details. The communication cost is similar to what is reported in the submission.

## Overhead of sketching for FSS-based PB-iFSS
Another benefit of using FSS to realize Property-based-iFSS is the fact that the servers can verify clients' keys, to make sure that the clients' inputs are well-formed. 
More specifically, the clients' inputs need to be:
- In range for $L_\infty$: The basic idea of FSS for $L_\infty$ is that the clients share intervals of radius $\delta$.
Sketching not only make sure that the sharings are actually for intervals, but also checks that these intervals indeed have radius $\delta$.
- Well-formed for $L_1$, $L_2$: The FSS for $L_p$ are much more complicated, but their general idea is we share many intervals with the _same center_ and _correlated radii_ (like $\delta, \delta^2, \ldots$). 
We can also check this with the awesome sketching.

The _communication overhead_ of sketching is super small, in fact, we batch everything together. 
The _total_ cost for clients to both share and sketch their points is reported in the previous subsection.
The following table shows the _computation overhead_ of sketching for the two servers to verify keys from _all clients_:

<div align="center">

| distance metric | $L_{\infty}$ | $L_1$ | $L_2$ |
| --- | ---: | ---: | ---:|
| Sketching **busiest_day** | $6.86$ s | $30.0$ s | $39.2$ s |
| Sketching **busiest_week** | $19.0$ s | $103$ s | $116$ s |
| Sketching **busiest_month** | $38.0$ s | $246$ s | $267$ s |

</div>

## Large-scale runs
We choose to run only options with Garbled Circuits for both distance checking and threshold comparison for the larger dataset: **busiest_week** and **busiest_month**.
The dealer does not participate in these runs with Garbled Circuits.
- **Scalable**: The fastest option in LAN returns the set of Fuzzy Heavy Hitters in around $2$ minutes for a dataset of $115K$ points.
The bandwidth consumption is also small, with one server sending $9$ GB and the other server sending $39$ GB.
- **Less client work with OKVS**: When using OKVS to initiate the PB-iFSS, the client's key size is much less. 
More specifically, the key size for $L_\infty$ is $2\times$ less than using FSS, while the advantage of OKVS for $L_1$ and $L_2$ are $6\times$ and $8\times$ accordingly.
So using OKVS can have an advantage when the servers are much more powerful in terms of computation and networking.
- **FSS-based PB-iFSS has the best LAN runtime**: Using FSS-based PB-iFSS, in combination with Garbled Circuits, it takes correspondingly $2$ minutes, $6$ minutes, and $8$ minutes to get the data analysis for $L_\infty$, $L_1$, and $L_2$, for the **busiest_month** dataset with $115K$ points.
Furthermore, the code is _embarrassingly parallelizable_, and may have stella running time (and even smaller cloud computing cost) if implemented with GPUs.
- **Some breakdown about each component**: The client cost of generating shares and the servers' cost of doing threshold comparison are much smaller than the cost for the distance checking step. 
In short, for each point that the servers want to test, they have to obtain secret sharing of its distance against $n$ other points, where $n$ is the number of clients.
The length of input to this step (which is $h_2$, refer to [Parameters](#parameters) for more details) is hence the most crucial factor for the performance of the protocol.
This also explains the better running time for FSS-based sharing compared to OKVS-based sharing, since FSS-based sharing does not need long output to guarantee correctness.

<div align="center">

<table>
  <tr>
    <th>Dataset</th>
    <th>Distance Metric</th>
    <th>FSS</th>
    <th>Method</th>
    <th>total client send</th>
    <th>total client time</th>
    <th>server0 send</th>
    <th>server1 send</th>
    <th>time</th>
  </tr>
  <tr>
    <td rowspan="6"><strong>busiest_week</strong></td>
    <td rowspan="2"><span style="font-style: italic;">L</span><sub>&infin;</sub></td>
    <td >FSS</td>
    <td>gc_gc</td>
    <td>526 MB</td>
    <td>7.99 s</td>
    <td>9.02 GB</td>
    <td>38.5 GB</td>
    <td>72.2 s</td>
  </tr>
  <tr>
    <td>OKVS</td>
    <td>gc_gc</td>
    <td>232 MB</td>
    <td>17.4 s</td>
    <td>333 GB</td>
    <td>1,661 GB</td>
    <td>967 s</td>
  </tr>
  <tr>
    <td rowspan="2"><span style="font-style: italic;">L</span><sub>1</sub></td>
    <td>FSS</td>
    <td>gc_gc</td>
    <td>1.40 GB</td>
    <td>18.2 s</td>
    <td>8.78 GB</td>
    <td>80.1 GB</td>
    <td>188 s</td>
  </tr>
  <tr>
    <td>OKVS</td>
    <td>gc_gc</td>
    <td>233 MB</td>
    <td>17.5 s</td>
    <td>99.5 GB</td>
    <td>809 GB</td>
    <td>484 s</td>
  </tr>
  <tr>
    <td rowspan="2"><span style="font-style: italic;">L</span><sub>2</sub></td>
    <td>FSS</td>
    <td>gc_gc</td>
    <td>1.89 GB</td>
    <td>22.5 s</td>
    <td>15.2 GB</td>
    <td>133 GB</td>
    <td>253 s</td>
  </tr>
  <tr>
    <td rowspan="1">OKVS</td>
    <td>gc_gc</td>
    <td>233 MB</td>
    <td>17.4 s</td>
    <td>128 GB</td>
    <td>1,041 GB</td>
    <td>590 s</td>
  </tr>
  <tr>
    <td rowspan="6"><strong>busiest_month</strong></td>
    <td rowspan="2"><span style="font-style: italic;">L</span><sub>&infin;</sub></td>
    <td >FSS</td>
    <td>gc_gc</td>
    <td>1.26 GB</td>
    <td>15.6 s</td>
    <td>16.9 GB</td>
    <td>72.2 GB</td>
    <td>127 s</td>
  </tr>
  <tr>
    <td>OKVS</td>
    <td>gc_gc</td>
    <td>453 MB</td>
    <td>34.1 s</td>
    <td>647 GB</td>
    <td>3,225 GB</td>
    <td>1,866 s</td>
  </tr>
  <tr>
    <td rowspan="2"><span style="font-style: italic;">L</span><sub>1</sub></td>
    <td>FSS</td>
    <td>gc_gc</td>
    <td>2.73 GB</td>
    <td>35.4 s</td>
    <td>16.9 GB</td>
    <td>154 GB</td>
    <td>352 s</td>
  </tr>
  <tr>
    <td>OKVS</td>
    <td>gc_gc</td>
    <td>454 MB</td>
    <td>34.1 s</td>
    <td>198 GB</td>
    <td>1,611 GB</td>
    <td>943 s</td>
  </tr>
  <tr>
    <td rowspan="2"><span style="font-style: italic;">L</span><sub>2</sub></td>
    <td>FSS</td>
    <td>gc_gc</td>
    <td>3.69 GB</td>
    <td>43.8 s</td>
    <td>29.4 GB</td>
    <td>257 GB</td>
    <td>480 s</td>
  </tr>
  <tr>
    <td rowspan="4">OKVS</td>
    <td>gc_gc</td>
    <td>454 MB</td>
    <td>34.4 s</td>
    <td>647 GB</td>
    <td>3,225 GB</td>
    <td>1,866 s</td>
  </tr>
</table>

</div>

_Remarks_: There is in fact a stalling bug when we run for $32$ cores, for the **busiest_week** sub-dataset, with OKVS as the function secret sharing option and using FSS for both distance checking and threshold comparison, that we currently do not know how to investigate, as it only happens at large scale. Please try to run this experiment if you have the setup, and don't be hesitate to contact us about the bug.

## Parameters
We choose computational security parameter $\kappa = 128$ and statistical security parameters $\lambda = 40$. 

1. **OKVS**: We use RB-OKVS, and choose a conservative choice of parameters in order to achieve $\lambda = 40$ statistical parameter for failure rate.
For more details, we set the OKVS rate to be $\epsilon = 0.1$, which means the number of columns is $1.1\times$ the number of rows. 
Furthermore, the number of columns is always at least $60$.
About the band-width, we set the band-width to be a fixed $100$, and to be exactly the number of columns in case this quantity is smaller than $100$.
This achieves the OKVS encoding failure probability $2^{-40}$ according to the [RB-OKVS](https://www.usenix.org/conference/usenixsecurity23/presentation/bienstock) paper.

2. **Input length** $h_1$: The input length just needs to be large enough to represent each coordinate. 
In the cleaned Ride Austin dataset, each coordinate can be represented by $11$ bits, hence we set $h_1 = 11$ in all of our experiments.

3. **FSS output** $h_2$: The FSS output for the _FSS-based_ approach is set differently from the _OKVS-based_ approach.
- _FSS-based_ approach: As correctness is guarantee for all points in the domain, we do not need to account for failure probability, hence the output simple needs to not cause overflow when comparing with the distance threshold.
For example, for distance metric $L_2$ with threshold $\delta = 5$, and there are $2$ dimensions, we can bound the FSS (distance) output dimension with $5^2 + 1$, and the maximum total distance calculated is $2 \times (5^2 + 1) = 52$, which can be represented in $6$ bits.
In our experiments for the unknown dictionary setting, we choose $\delta = 5$, and we only run the experiments for the case when the number of dimensions is $d = 2$. 
In the following table, we provide suggested $h_2$ parameter in the FSS-based approach, for number of dimensions $d=2$ and $d=4$.

<div align="center">

| distance metric | $L_{\infty}$ | $L_1$ | $L_2$ |
| --- | ---: | ---: | ---:|
| $h_2$ when $d=2$ | $1$ | $4$ | $6$ |
| $h_2$ when $d=4$ | $1$ | $5$ | $7$ |

</div>

- _OKVS-based_ approach: Choosing $h_2$ for the OKVS-based approach is more tricky due to the correctness probability. 
For keys that are not encoded in the OKVS, the decoding output is a random value, and with a small probability, the summed-up distance would still indicate that $d(x, y) \le \delta$ (which is wrong).
For example, when the output range is $2^{h_2}$, and we consider the distance metric $L_2$, with radius $\delta = 5$, the random summed-up distance can be a false-positive with probability $5^2 / 2^{h_2}$.
We further union-bound this probability with the total number of times that an OKVS is evaluated, which we estimate to be the number of clients.
Again, in the $L_2$ distance metric example, this probability would be estimated as $5^2 n / 2^{h_2}$, and we choose $h_2 \ge \log(25 n) + \kappa$ to satisfy the statistical security parameter.
The full table for the parameter $h_2$ that we chose for the experiments is shown below.

<div align="center">

| distance metric | $L_{\infty}$ | $L_1$ | $L_2$ |
| --- | ---: | ---: | ---:|
| $n = 21,581$ | $h_2 = 55$ | $h_2 = 58$ | $h_2 = 60$ |
| $n = 59,040$ | $h_2 = 56$ | $h_2 = 59$ | $h_2 = 61$ |
| $n = 115,174$ | $h_2 = 58$ | $h_2 = 61$ | $h_2 = 63$ |

</div>

4. **Fuzzy match output** $h_3$: The fuzzy match output is a mod $2^{h_3}$ value, but with value being only either $0$ (if $d(x, y)$ exceeds $\delta$, which means the two points are not close, hence not a fuzzy match) or $1$.
For each point $x$ in consideration, it is fuzzy-matched with exactly $n$ other points $y$, where $n$ is the number of clients, hence aggregating the results return a value that does not exceed $n$.
We just need to choose $h_3$ such that $2^{h_3} \ge n$. 
The reference for the parameter $h_3$ chosen in our experiments for the RideAustin dataset is included in the following table.

<div align="center">

| Dataset | busiest_day | busiest_week | busiest_month |
| --- | ---: | ---: | ---:|
| Number of client points | $21,581$ | $59,040$ | $115,174$ |
| Fuzzy match output $h_3$ | $15$ | $16$ | $18$ |

</div>



# Installation

This code has been tested with `Ubuntu 24.04.4 LTS`.

Dependencies:
- Rust: this code has been tested with `Rust 1.96.0` (ac68faa20 2026-05-25)
- All dependencies in Cargo.toml has been up-to-date at the moment of publishing this artifact.

Please refer to the following guide to install Rust in Ubuntu: [Digital Ocean Guide](https://www.digitalocean.com/community/tutorials/install-rust-on-ubuntu-linux).

After getting all the dependencies, to compile, simply run `cargo build`:
```
cargo build --release
```

# How to run Mosaic
There are four parties involved in this protocol: `server0`, `server1`, `client` and `dealer`. 
Before running the protocol, you first need to prepare a list of clients' points, a main config file, and a separate network config file.

## Prepare data
Simply create a json file, for example, the following json file contains two client points:
```javascript
[
  [
    XXX,
    XXX
  ],
  [
    XXX,
    XXX
  ]
]
```    

In our experiments, we clean the RideAustin dataset and obtain three sub-datasets with different sizes, in order to test the scalability of Mosaic. 
Each sub-dataset is stored in a `.csv`, that is available in the `data` folder in the Zenodo repository of Mosaic. 
For example, the csv file for the rides in the busiest day (the other two datasets are for the busiest week and the busiest month) can be found at `data/sample_busiest_day.csv` on Zenodo.

The .csv files are then extracted into json files that contain a list of $2$-dimensional points (for the starting points of rides) or $4$-dimensional points (for the start and end points of rides), using the code in `src/bin/data/ride_austin_json_generator.rs`.
We can run the json file generator from the csv file with the following command:
```
cargo run --release --bin ride_austin_json_generator -- 
--input (input_csv) --output (output_json) 
--output-type (output_type)
```
Here, the `output-type` is $0$ if you want $2$-dimensional points (corresponding to the start of a ride), and $1$ if you want $4$-dimensional points (corresponding to the start and the end of a ride).

To sample a dictionary for the servers to rely on in the _known dictionary_ setting, use `ride_austin_json_sample.rs`.
```
cargo run --release --bin ride_austin_json_sampler --
--input (input_csv) --output (output_json) 
--query-num (dictionary_size)
--output-type (output_type)
```
All prepared .json files for our experiments are available on Zenodo, in the same `data` folder. 

### Location encodings
The generator and the sampler take `--encoding` to choose how a location becomes integer coordinates.
Mosaic compares points coordinate by coordinate, so an encoding has to give coordinates whose differences track physical distance; `delta` is then measured in cells of that encoding.
The protocol itself does not change, only `d` (coordinates per point), `h1` (bits per coordinate) and the parameters that depend on them.

| `--encoding` | Coordinates | `d`, `h1` | Outside the area | Default `delta` (Linf / L1 / L2) |
| --- | --- | --- | --- | --- |
| `mosaic-grid` (default) | `[lat, lon]` on the original 0.001 degree grid | 2, 11 | clamped to the edge | 5 / 5 / 5 |
| `morton` | `[lat, lon]` cell indices in the `--morton-*` box, `2^axis_bits` cells per axis | 2, `axis_bits` (default 10) | dropped | 5 / 5 / 5 |
| `geohash` | `[lat, lon]` indices from the `--geohash-bits` bits below `--geohash-prefix` (default 21 below `9v6`) | 2, 11 | dropped | 5 / 5 / 5 |
| `h3` | cube coordinates `[i, j, i - j]` of the `--h3-resolution` (default 10) hexagon around `--h3-origin-*` | 3, 11 | dropped | 4 / 8 / 5 |

Next to the points, the generator writes `<output>.encoding.json` with the encoding, `d`, `h1` and how many rides were kept, clamped and dropped.
A dropped ride is not sent, so `num_clients` and the 2% `match_threshold` come from the kept rides.

The original grid needs nothing new: it keeps using `data/start_busiest_<dataset>.json` and the configs in `configs/unknown_dictionary`.
Running the generator with the default `--encoding mosaic-grid` writes the same JSON as before, plus the `.encoding.json` that the decoder reads.

For another encoding, generate its datasets and configs (`<encoding>` is the `--encoding` value, e.g. `geohash`):
```bash
mkdir -p data/encodings/<encoding>
for B in day week month; do
  cargo run --release --bin ride_austin_json_generator -- \
    --input data/sample_busiest_${B}.csv --output data/encodings/<encoding>/start_busiest_${B}.json \
    --output-type 0 --encoding <encoding>
done
python3 src/data_toolkit/make_encoding_configs.py <encoding>
```
The configs are written to `configs/encodings/<encoding>/busiest_<dataset>/`, and `ENCODING=<encoding> ./run_exp.sh <role>` runs them.
Without `ENCODING` (or with `ENCODING=mosaic_grid`), `run_exp.sh` runs `configs/unknown_dictionary` as before.

**Morton.** Mastic's Morton encoder interleaves the two cell indices into one bit string. Mosaic's crawl already takes one bit of each coordinate in turn, so here the indices are sent as they are and Morton is a grid over a chosen box.
The default box (29.9005 to 30.9245, -98.0505 to -97.0265, 10 bits per axis) puts the cell edges half a step off the 3-decimal RideAustin coordinates, so its cells are exactly Mosaic's grid cells shifted by (634, 693), with one bit less per coordinate.
Its heavy hitters are the `mosaic-grid` ones shifted by the same amount (916 / 426 / 598 on the busiest day), and no rides are dropped.
Other cell sizes, such as Mastic's `morton-h3res10-area` box (`--morton-max-lat 31.199435808 --morton-max-lon -96.751564192`), only need different `--morton-*` flags.

**Geohash.** Geohash cells form a uniform grid, so the bits below the prefix are split by position into a latitude and a longitude index.
All of Austin lies in `9v6` (15 bits); the 21 bits below it give 11 latitude and 10 longitude bits, and cells of 0.000687 by 0.001373 degrees (about 76 m by 132 m, the closest geohash area to Mosaic's cells).
Mosaic uses one `delta` for every coordinate, so on these cells the neighbourhood is about 1.7 times wider east-west than north-south; an odd number of bits below `9v6` (e.g. `--geohash-bits 20`) gives cells that are square in degrees, like Mosaic's.
On the busiest day it finds 891 / 399 / 525 heavy hitters with no rides dropped.

**H3.** The H3 index digits that Mastic sends cannot be used as coordinates, because neighbouring hexagons can have very different indices.
Instead, each cell is given by its local `(i, j)` coordinates relative to the cell containing the origin (downtown Austin by default), sent as the three cube coordinates `[i, j, i - j]`, each shifted by `2^(h1 - 1)`.
On these coordinates `Linf` is exactly the H3 grid distance (number of hexagon steps), `L1` is twice the grid distance, and `L2` is `sqrt(2)` times the distance between cell centers in units of the center spacing (about 140 m at resolution 10).
The defaults `delta` = 4 / 8 / 5 give neighbourhoods of about 560 m, comparable to the other encodings; `Linf` 4 and `L1` 8 are the same neighbourhood.

The crawl does not know that the third coordinate is `i - j`, so it also reports triples that are not cells: on the busiest day 5018 `Linf` heavy hitters, of which 445 are cells.
`heavy_hitters_decoder` leaves the others out. They also make the crawl about 3 times longer (13953 prefixes instead of 4257 if the servers dropped every prefix that cannot contain a cell, which they could do from the public prefixes alone; this is not implemented).

`mosaic_others ground-truth` writes the heavy hitters as integer points; `heavy_hitters_decoder` turns them back into cells and locations:
```bash
cargo run --release --bin heavy_hitters_decoder -- \
  --input results.json --metadata data/start_busiest_day.encoding.json --output heavy_hitters.csv
```

## Prepare the config file
First, prepare a network config file. 
We usually put this file in `configs/network_config.json`.
We recommend putting the port numbers quite far from each other, as if there are $m$ threads, we will use $m$ consecutive port numbers for each usage. 
For example, if you set "server0_to_server1_port" being $3000$, and there are $32$ threads, server0 and server1 will talk through ports ranging from $3000$ to $3031$.
```javascript
{
  "network": {
    "server0_addr": "XXX.XX.XX.XX",
    "server1_addr": "XXX.XX.XX.XX",
    "server0_to_server1_port": XXXX,
    "dealer_to_server0_port": XXXX,
    "dealer_to_server1_port": XXXX,
    "client_to_server0_port": XXXX,
    "client_to_server1_port": XXXX
  }
}
```
There are two more config files that we need to prepare. 
The first one is a data-related config file, that will include the source to data files, and also all parameters needed in the protocol.
We include the config for all of our experiment runs in the **configs** folder. 
For the _unknown dictionary_ setting, we put them in **configs/unknown_dictionary**.
```javascript
{
  "data_file": "path_to_client_points",
  "query_file": "path_to_server_points_if_apply",
  "protocol": {
    "delta": XXX, // distance threshold 
    "threshold": XXX, // count threshold
    "h1": XX, // large enough to represent every input number
    "h2": XX, // FSS output, dependent on whether the function secret sharing is "FSS" or "OKVS"
    "h3": XX, // Count output, large enough to count
    "d": X, // Number of dimensions
    "share_method": "FSS", // "FSS" or "OKVS"
    "dictionary_type": "Unknown", // "Known" or "Unknown"
    "check_property": "Equality", // "Equality" or "MuBounded"
    "distance_metric": "Linf", // "Linf", "L1", "L2", or "L3"
    "num_clients": XXXX // Number of client points
  },
  "output": {
    "verbose": true,
    "show_intermediate": false,
    "output_file": "results.json"
  }
}
```
Finally, we need to prepare a config file to state the method that we want to use to run _distance checking_ and _threshold comparison_. 
Both steps support two methods: either 2PC-based (using **Garbled Circuits**) or Dealer-based (using **Function Secret Sharing**).
```javascript
{
  "check_method": "GC", // "GC" or "FSS"
  "threshold_method": "GC" // "GC" or "FSS"
}
```


## Running the Mosaic protocol

Open four terminals (or four different machines, that can talk to each other through Tcp).
Please run the following four command lines for the four simulated parties:
- `server0`
```
cargo run --release --bin mosaic_server -- --side 0 
--config (path_to_config) 
--network-config (path_to_network_config) --method-config (path_to_method_config) 
--threads (num_threads)
```
- `server1`
```
cargo run --release --bin mosaic_server -- --side 1 
--config (path_to_config) 
--network-config (path_to_network_config) --method-config (path_to_method_config) 
--threads (num_threads)
```
- `client`
```
cargo run --release --bin mosaic_client -- 
--config (path_to_config) --network-config (path_to_network_config)
```
- `dealer`
```
cargo run --release --bin mosaic_dealer -- 
--config (path_to_config) --network-config (path_to_network_config)
--threads (num_threads)
```

Example:
```bash
cargo run --release --bin mosaic_server -- --side 0 --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json --method-config configs/method/gc_gc.json --threads 32
```

```bash
cargo run --release --bin mosaic_server -- --side 1 --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json --method-config configs/method/gc_gc.json --threads 32
```

```bash
cargo run --release --bin mosaic_client -- --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json
```

```bash
cargo run --release --bin mosaic_dealer -- --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json --threads 32
```

Command line parameters:
- `config`: All four commands need a config parameter, please provide the path to the main config file that you prepared in the [Prepare Config](#prepare-the-config-file) section.
- `network-config`: The Mosaic and naive servers need the separate network config file. Use `configs/network_config.json` or point this flag at your own copy.
- `method-config`: The server method config file. Use `configs/method_config.json` or point this flag at your own copy.
- `threads`: Specify the number of threads that the two servers and the dealer use.
Currently we only tested the code for the case when the number of threads used by all these three parties are the same.
So please set `threads` to be the same in all three commands.

## Running the naive solution
We also implement a Naive solution, that is, each client sends out all the points inside the multi-dimensional ball of radius $\delta$, and run Poplar on all these points. 
This solution does not have a good performance.

You can reuse the same main config file and the same `configs/network_config.json` file from the Mosaic setup.
Run the servers first, then start the client:

- `server0`
```bash
cargo run --release --bin naive_server -- --side 0 --config (path_to_config) --network-config configs/network_config.json --num-threads (num_threads)
```

- `server1`
```bash
cargo run --release --bin naive_server -- --side 1 --config (path_to_config) --network-config configs/network_config.json --num-threads (num_threads)
```

- `client`
```bash
cargo run --release --bin naive_client -- --config (path_to_config) --network-config configs/network_config.json
```

Example:
```bash
cargo run --release --bin naive_server -- --side 0 --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json --num-threads 32
```

```bash
cargo run --release --bin naive_server -- --side 1 --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json --num-threads 32
```

```bash
cargo run --release --bin naive_client -- --config configs/known_dictionary/busiest_day_linf_fss_config_known.json --network-config configs/network_config.json
```

## Running the ground truth
The `mosaic_others` binary runs the plaintext version of the protocol locally, so you can compare the secure end-to-end implementation against an exact answer.

This binary has a single subcommand:

```bash
cargo run --release --bin mosaic_others -- ground-truth --config (path_to_config)
```

The configuration file is the same one used by the distributed Mosaic binaries. It should point to the client dataset, and for the known-dictionary case it should also point to the query set.

What it does:
- Loads the client points from `config.data_file`
- Loads query points for the known-dictionary case from `config.query_file`
- Computes exact fuzzy heavy hitters in plaintext
- Prints a summary of the result
- Optionally writes the result to `config.output.output_file` when that field is set

Examples:
```bash
cargo run --release --bin mosaic_others -- ground-truth --config configs/unknown_dictionary/busiest_day/linf_fss_config.json
```

If `config.output.output_file` is present, the binary writes a JSON file containing the exact heavy hitters and a small summary, which makes it easy to compare against the distributed protocol output.

# Optimizations
Since the time of publication, there has been two optimizations that makes the code run much faster now and also consumes less RAM. 

1. In the original code, we in fact forgot to parallelize the _FSS evaluation_ step in each loop iteration. 
Putting parallelization in this part reduces $8\times$ the running time compared to what was shown in Table 4 in the submission.

2. The original code stores every FSS evaluations in each iteration of the loop, with the goal in mind to be faster FSS evaluation for the next prefix. 
However, later in the run, the number of candidate prefixes grows into a large number, which causes the amount of FSS evaluations stored in RAM becoming to large ($>140$ GB).
In this new version, we implement the binary tree expansion in a more "streaming" fashion, which reduces the peak memory consumption into less than $20$ GB for **busiest week**. 
This implementation also introduces a trade-off, with more RAM consumption might be traded for even faster runs.

3. The memory consumption for the dealer is also implemented much more carefully. We have experimented and confirmed that the dealer can be run for **busiest month**, with the PB-iFSS option being OKVS, and methods being fss_fss (the most memory-expensive option, since the dealer needs to store all the FSS key pairs for the distance checking phase), using `c5a.2xlarge` (peak memory consumption 160 GB, taking $4,012$ seconds).


# Authors
Gayathri Garimella, _Brown University_

Peihan Miao, _Brown University_

Eileen Nolan, _Brown University_

Phuoc Van Long Pham, _Brown University_

Siddarth Sitaraman, _Brown University_

For more information about the implementation, please contact Phuoc: phuoc_van_long_pham@brown.edu.
