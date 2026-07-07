use std::{env, net::IpAddr};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use futures::{StreamExt, stream::FuturesUnordered};
use porkbun_api::{
    ApiKey, Client, CreateOrEditDnsRecord, DnsRecordType, transport::DefaultTransport,
};

/// TODO
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// domain(s) to update records for
    #[arg(required = true)]
    domain: Vec<String>,

    /// set A records
    #[arg(long)]
    ipv4: bool,

    /// set AAAA records
    #[arg(long)]
    ipv6: bool,

    /// TTL in seconds
    #[arg(long, default_value_t = 21600)]
    ttl: u32,
}

const SECRET: &str = "PORKBUN_API_SECRET";
const KEY: &str = "PORKBUN_API_KEY";

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !(args.ipv4 || args.ipv6) {
        return Err(anyhow!("no record types selected, pass --ipv4 or --ipv6"));
    }
    let api_key =
        ApiKey::new(env::var(SECRET).context(SECRET)?, env::var(KEY).context(KEY)?);
    let client = Client::new(api_key);

    let mut ips = Vec::new();
    if args.ipv4 {
        ips.push(IpAddr::V4(
            reqwest::get("https://api.ipify.org").await?.text().await?.parse()?,
        ));
    }
    if args.ipv6 {
        ips.push(client.ping().await?);
    }
    let mut updates = FuturesUnordered::new();
    for ip in ips {
        for domain in &args.domain {
            updates.push(update(ip, domain, &client))
        }
    }
    while let Some(edit) = updates.next().await {
        let (domain, ip) = edit?;
        println!("Updated {domain} to point at {ip}");
    }
    Ok(())
}

async fn update<'a>(
    ip: IpAddr,
    domain: &'a str,
    client: &Client<DefaultTransport>,
) -> Result<(&'a str, IpAddr)> {
    let update = CreateOrEditDnsRecord::A_or_AAAA(None, ip);
    let records = &client.get_all(domain).await?;
    let record = records
        .iter()
        .find(|d| {
            d.name == domain
                && (ip.is_ipv4() && d.record_type == DnsRecordType::A
                    || d.record_type == DnsRecordType::AAAA)
        })
        .context("no matching records")?;

    client.edit(domain, &record.id, update).await?;
    Ok((domain, ip))
}
