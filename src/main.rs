use anyhow::{Context, Result, anyhow};
use clap::Parser;
use futures::{StreamExt, stream::FuturesUnordered};
use local_ip_address::local_ipv6;
use porkbun_api::{
    ApiKey, Client, CreateOrEditDnsRecord, DnsRecordType, transport::DefaultTransport,
};

use std::{
    env,
    net::{IpAddr, Ipv6Addr},
    time::Duration,
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None, group(
    clap::ArgGroup::new("ips").required(true).multiple(true)
))]
struct Args {
    /// domain(s) to update records for
    #[arg(required = true)]
    domain: Vec<String>,

    /// set A records
    #[arg(long, short = '4', group = "ips")]
    ipv4: bool,

    /// set AAAA records
    #[arg(long, short = '6', group = "ips")]
    ipv6: bool,

    /// set A or AAAA records using the given ip address rather than your current ip
    #[arg(long, group = "ips", conflicts_with_all = ["ipv4", "ipv6"])]
    ip: Option<String>,

    /// TTL in seconds
    #[arg(long, default_value_t = 21600)]
    ttl: u64,
}

const SECRET: &str = "PORKBUN_API_SECRET";
const KEY: &str = "PORKBUN_API_KEY";

struct Update<'a> {
    old: String,
    new: IpAddr,
    domain: &'a str,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let api_key =
        ApiKey::new(env::var(SECRET).context(SECRET)?, env::var(KEY).context(KEY)?);
    let client = Client::new(api_key);

    let mut ips = Vec::new();
    if let Some(ip) = args.ip {
        ips.push(ip.parse()?)
    } else {
        if args.ipv4 {
            ips.push(IpAddr::V4(
                reqwest::get("https://api.ipify.org").await?.text().await?.parse()?,
            ));
        }
        if args.ipv6 {
            // TODO: iterate over local ipv6s and pick the right one
            let IpAddr::V6(local) = local_ipv6()? else { unreachable!() };
            let mac = mac_address::get_mac_address()?.context("no interfaces")?;
            let [a, b, c, d, e, f] = mac.bytes();
            let ip = local.to_bits() & 0xFFFF_FFFF_FFFF_FFFF_0000_0000_0000_0000
                | u64::from_be_bytes([a ^ 2, b, c, 0xff, 0xfe, d, e, f]) as u128;
            ips.push(dbg!(IpAddr::V6(Ipv6Addr::from_bits(ip))));
        }
    }

    let mut updates = FuturesUnordered::new();
    for ip in ips {
        for domain in &args.domain {
            updates.push(update(&client, ip, domain, Duration::from_secs(args.ttl)))
        }
    }
    while let Some(edit) = updates.next().await {
        let Update { old, new, domain } = edit?;
        println!("Updated {domain} to point at {new} (was {old})");
    }
    Ok(())
}

async fn update<'a>(
    client: &Client<DefaultTransport>,
    ip: IpAddr,
    domain: &'a str,
    ttl: Duration,
) -> Result<Update<'a>> {
    let without_tld = &domain[..domain.rfind('.').context("no dot in domain")?];
    let root = &domain[without_tld.rfind('.').map(|n| n + 1).unwrap_or_default()..];
    let subdomain = without_tld.rfind('.').map(|n| &domain[..n]);
    let update = CreateOrEditDnsRecord::A_or_AAAA(subdomain, ip).with_ttl(Some(ttl));
    let records = &client.get_all(root).await?;
    let record = records
        .iter()
        .find(|d| {
            d.name == domain
                && (ip.is_ipv4() && d.record_type == DnsRecordType::A
                    || d.record_type == DnsRecordType::AAAA)
        })
        .context("no matching records")?;

    if record.content.parse::<IpAddr>()? == ip {
        return Err(anyhow!("{domain} already points at that ip"));
    }
    client
        .edit(root, &record.id, update)
        .await
        .inspect_err(|e| eprintln!("\x1b[;2m{e:?}\x1b[0m"))
        .ok();
    Ok(Update { domain, old: record.content.clone(), new: ip })
}
