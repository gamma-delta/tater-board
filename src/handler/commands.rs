//! Handles the commands

use std::convert::TryFrom;

use anyhow::anyhow;
use serenity::{
    client::Context,
    model::{
        channel::Message,
        channel::ReactionType,
        id::{ChannelId, UserId},
        Permissions,
    },
    prelude::*,
};

use super::{Handler, HandlerWrapper};

pub async fn handle_commands(
    wrapper: &HandlerWrapper,
    ctx: &Context,
    uid: UserId,
    message: &Message,
) -> Result<(), anyhow::Error> {
    let guild_id = match message.guild_id {
        Some(it) => it,
        None => return Ok(()),
    };
    let mut handlers = wrapper.handlers.lock().await;
    let this = handlers.entry(guild_id).or_insert_with(Handler::new);

    if message.author.id == uid
        || !message.content.starts_with(&this.config.trigger_word)
    {
        return Ok(());
    }

    // Check if they are an admin
    let guild = match message.guild(&ctx.cache).await {
        Some(it) => it,
        None => return Ok(()),
    };
    let is_admin = this.config.admins.contains(&message.author.id)
        || match guild
            .member(&ctx.http, message.author.id)
            .await?
            .roles(&ctx.cache)
            .await
        {
            Some(roles) => roles
                .iter()
                .any(|r| r.has_permission(Permissions::ADMINISTRATOR)),
            None => return Ok(()),
        };

    let split = message.content.split_whitespace().collect::<Vec<_>>();
    if split.len() < 2 {
        return Ok(());
    }
    let cmd = split[1];
    let args = &split[2..];

    match cmd {
        "help" => {
            const HELP: &str = r" === PotatoBoard Help ===
- `help`: Get this message.
- `receivers <page_number>`: See the most protatolific receivers of potatoes. `page_number` is optional.
- `givers <page_number>`: See the most protatolific givers of potatoes. `page_number` is optional.";
            const ADMIN_HELP: &str = r"You're an admin! Here's the admin commands:
- `set_pin_channel <channel_id>`: Set the channel that pinned messages to go, and adds it to the potato blacklist.
- `set_potato <emoji>`: Set the given emoji to be the operative one.
- `set_threshold <number>`: Set how many potatoes have to be on a message before it is pinned.
- `blacklist <channel_id>`: Make the channel no longer eligible for pinning messages, regardless of potato count.
- `unblacklist <channel_id>`: Unblacklist this channel so messages from it can be pinned again.
- `show_blacklist`: Show which channels are ineligible for pinning messages.
- `admin <user_id>`: Let this user access this bot's admin commands on this server.
- `unadmin <user_id>`: Stops this user from being an admin on this server.
- `list_admins`: Print a list of admins.
- `save`: Flush any in-memory state to disk.
People with any role with an Administrator privilege are always admins of this bot.";
            message.channel_id.say(&ctx.http, HELP).await?;
            if is_admin {
                message.channel_id.say(&ctx.http, ADMIN_HELP).await?;
            }
        }
        leaderboard @ "receivers" | leaderboard @ "givers" => {
            const PAGE_SIZE: usize = 10;
            let res: Result<(), String> = try {
                let page_num = args.get(0).and_then(|page| page.parse().ok()).unwrap_or(0);

                let map = if leaderboard == "receivers" {
                    &this.taters_got
                } else {
                    &this.taters_given
                };
                // high score at the front
                let mut scores: Vec<_> = map.iter().map(|(id, count)| (*id, *count)).collect();
                scores.sort_by_key(|(_id, count)| *count);
                scores.reverse();
                // de-mut it
                let scores = scores;

                let to_display = scores
                    .iter()
                    .enumerate()
                    .skip(PAGE_SIZE * page_num)
                    .take(PAGE_SIZE);

                let mut board = String::with_capacity(20 * PAGE_SIZE);
                let verb = if leaderboard == "receivers" {
                    "received"
                } else {
                    "given"
                };
                for (idx, (user_id, count)) in to_display {
                    let medal = match idx + 1 {
                        1 => "🏅",
                        2 => "🥈",
                        3 => "🥉",
                        _ => "🎖️",
                    };
                    let user = ctx
                        .http
                        .get_user(user_id.0)
                        .await
                        .map_err(|e| e.to_string())?;

                    board.push_str(&format!(
                        "{} {}: {} has {} {}x taters\n",
                        medal,
                        idx + 1,
                        user.mention(),
                        verb,
                        count,
                    ));
                }

                let asker_place = scores
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, (id, score))| {
                        if *id == message.author.id {
                            Some((idx + 1, score))
                        } else {
                            None
                        }
                    })
                    .next();
                let (place, score) = match asker_place {
                    Some((p, s)) => (p.to_string(), s.to_string()),
                    None => ("?".to_string(), "?".to_string()),
                };
                let total_pages = map.len() / PAGE_SIZE + 1;
                let footer = format!(
                    "Your place: #{}/{} with {}x {} | Page {}/{}",
                    place,
                    map.len(),
                    score,
                    this.config.tater_emoji.to_string(),
                    page_num + 1,
                    total_pages
                );

                message
                    .channel_id
                    .send_message(&ctx.http, |m| {
                        m.embed(|e| {
                            e.title(format!("Leaderboard - Taters {}", verb))
                                .description(&board)
                                .footer(|f| f.text(footer))
                        })
                    })
                    .await
                    .map_err(|e| e.to_string())?;
            };
            if let Err(oh_no) = res {
                message
                    .channel_id
                    .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                    .await?;
            };
        }
        "set_pin_channel" if is_admin => {
            let msg: Result<String, String> = try {
                let channel_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let channel_id = ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
                this.config.pin_channel = channel_id;

                let existed = !this.config.blacklisted_channels.insert(channel_id);
                let channel_mention = channel_id.mention();
                if !existed {
                    format!(
                        "Set pins channel to `{}` and added it to the blacklist",
                        &channel_mention
                    )
                } else {
                    format!(
                        "Set pins channel to `{}`, and it was already blacklisted",
                        &channel_mention
                    )
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "set_threshold" if is_admin => {
            let msg: Result<String, String> = try {
                let threshold = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let threshold = threshold.parse::<u64>().map_err(|e| e.to_string())?;
                this.config.threshold = threshold;
                format!("Threshold changed to {}", threshold)
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "blacklist" if is_admin => {
            let msg: Result<String, String> = try {
                let channel_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let channel_id = ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
                let existed = !this.config.blacklisted_channels.insert(channel_id);

                let channel_mention = channel_id.mention();
                if !existed {
                    format!("Blacklisted `{}`", &channel_mention)
                } else {
                    format!("`{}` was already blacklisted", &channel_mention)
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "unblacklist" if is_admin => {
            let msg: Result<String, String> = try {
                let channel_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let channel_id = ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
                let existed = this.config.blacklisted_channels.remove(&channel_id);

                let channel_mention = channel_id.mention();
                if existed {
                    format!("Unblacklisted `{}`", &channel_mention)
                } else {
                    format!("`{}` was not blacklisted", &channel_mention)
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "show_blacklist" if is_admin => {
            let msg = this
                .config
                .blacklisted_channels
                .iter()
                .map(|c| format!("- {}", c.mention()))
                .collect::<Vec<_>>()
                .join("\n");
            message.channel_id.say(&ctx.http, msg).await?;
        }
        "set_potato" if is_admin => {
            let msg: Result<String, String> = try {
                let emoji = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let potato_react = ReactionType::try_from(*emoji).map_err(|e| e.to_string())?;
                let old_react = this.config.tater_emoji.to_string();
                this.config.tater_emoji = potato_react;
                format!("Set potato emoji to {} (from {})", emoji, old_react)
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "admin" if is_admin => {
            let msg: Result<String, String> = try {
                let user_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let user_id = UserId(user_id.parse::<u64>().map_err(|e| e.to_string())?);
                let existed = !this.config.admins.insert(user_id);
                if !existed {
                    format!("Added `{}` as a new admin", user_id)
                } else {
                    format!("`{}` was already an admin", user_id)
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "unadmin" if is_admin => {
            let msg: Result<String, String> = try {
                let user_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let user_id = UserId(user_id.parse::<u64>().map_err(|e| e.to_string())?);
                let existed = this.config.admins.remove(&user_id);
                if existed {
                    format!("Removed `{}` from being an admin", user_id)
                } else {
                    format!("`{}` was not an admin", user_id)
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "list_admins" if is_admin => {
            let msg: Result<String, String> = try {
                let mut msg = String::from("Admins:");
                for &id in this.config.admins.iter() {
                    let user = ctx.http.get_user(id.0).await.map_err(|e| e.to_string())?;
                    msg += format!("\n- {}", user.tag()).as_ref();
                }
                msg
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "save" if is_admin => {
            // we only need to save taters cause, as this is an admin command, config is about to get saved
            let msg = if let Some(id) = message.guild_id {
                HandlerWrapper::save_server_taters(&wrapper.save_dir_path, &*handlers, id)
                    .await
                    .map(|_| String::from("Saved this server's taters!"))
            } else {
                Err(anyhow!("There was no guild ID (are you in a PM?)"))
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        command => {
            message
                .channel_id
                .send_message(&ctx.http, |m| {
                    m.content(format!("Unknown command: {}", command))
                        .allowed_mentions(|a| a.empty_parse())}
                ).await?;
        }
    }

    if is_admin {
        // Assume that an admin command means we changed something about the config.
        // This could be done smarter but i don't care
        if let Some(id) = message.guild_id {
            HandlerWrapper::save_server_config(&wrapper.save_dir_path, &*handlers, id)
                .await
                .map_err(|e| anyhow!(e))?;
        } else {
            message
                .channel_id
                .say(
                    &ctx.http,
                    String::from(
                        "Unable to save config because there was no guild ID (are you in a PM?)",
                    ),
                )
                .await?;
        }
    }

    Ok(())
}
